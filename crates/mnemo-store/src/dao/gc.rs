//! Snapshot garbage collection (DESIGN §8.3 — MVP simplification).
//!
//! DESIGN punts the "freeze" step (rewriting `visible_from` for rows that
//! survive past a deleted snapshot) to a future RFC (§13). This MVP therefore
//! only collects **anonymous / working_tree snapshots**: by construction those
//! are ephemeral (DESIGN §6.1) and their version rows are dropped wholesale.
//! Commit / manual / filehash snapshots are never touched here.
//!
//! Algorithm:
//! 1. Pick victims: snapshots where `kind IN ('anonymous','working_tree')`,
//!    older than `now - keep_days*86400`, and ranked beyond
//!    `keep_anonymous_snapshots` (most-recent-first).
//! 2. For each victim, delete `*_version` rows referencing it
//!    (`visible_from = victim` OR `visible_until = victim`).
//! 3. Delete the snapshot row.
//! 4. `PRAGMA incremental_vacuum` to actually return pages to the OS.

use mnemo_core::CoreError;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// GC policy (DESIGN §8.1 defaults).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GcPolicy {
    /// Always preserve the most-recent N commit / filehash snapshots
    /// (informational here — MVP only touches anonymous-class snapshots).
    pub keep_commit_snapshots: u32,
    /// Always preserve the most-recent N anonymous / working_tree snapshots.
    pub keep_anonymous_snapshots: u32,
    /// Snapshots younger than this many days are kept regardless of count.
    pub keep_days: i64,
    /// How often the daemon's background timer fires (0 disables the timer).
    pub run_interval_hours: u64,
}

impl Default for GcPolicy {
    fn default() -> Self {
        Self {
            keep_commit_snapshots: 50,
            keep_anonymous_snapshots: 5,
            keep_days: 30,
            run_interval_hours: 6,
        }
    }
}

/// Summary of a single GC pass.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GcReport {
    /// Snapshot rows actually deleted.
    pub freed_snapshots: usize,
    /// `symbol_version` + `file_version` + `edge_version` rows deleted.
    pub freed_versions: usize,
    /// DB file size before GC, in bytes.
    pub db_size_before: u64,
    /// DB file size after GC + incremental_vacuum, in bytes.
    pub db_size_after: u64,
}

/// Run one GC pass on `conn`'s database. Returns counts of what was reclaimed.
/// File-backed connections also report DB size before / after.
pub fn run(conn: &mut Connection, policy: &GcPolicy) -> Result<GcReport, CoreError> {
    let db_path = conn.path().map(std::path::PathBuf::from);
    let db_size_before = db_path
        .as_deref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0);

    let cutoff = now_secs() - policy.keep_days * 86_400;

    // Step 1: enumerate victim snapshots. Ranked-by-recency tail beyond the
    // keep threshold AND older than the cutoff.
    let mut stmt = conn.prepare(
        "SELECT id FROM snapshot
         WHERE kind IN ('anonymous', 'working_tree')
           AND created_at < ?1
           AND id NOT IN (
               SELECT id FROM snapshot
               WHERE kind IN ('anonymous', 'working_tree')
               ORDER BY created_at DESC, id DESC
               LIMIT ?2
           )
         ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(
        params![cutoff, policy.keep_anonymous_snapshots as i64],
        |row| row.get::<_, i64>(0),
    )?;
    let victims: Vec<i64> = rows.filter_map(|r| r.ok()).collect();
    drop(stmt);

    if victims.is_empty() {
        return Ok(GcReport {
            freed_snapshots: 0,
            freed_versions: 0,
            db_size_before,
            db_size_after: db_size_before,
        });
    }

    // Step 2 + 3: drop version rows + snapshot rows in one transaction.
    let mut freed_versions = 0usize;
    let tx = conn.transaction()?;
    for snap in &victims {
        for table in &["symbol_version", "file_version", "edge_version"] {
            // Born-at-victim: delete unconditionally.
            let n = tx.execute(
                &format!("DELETE FROM {table} WHERE visible_from = ?1"),
                params![snap],
            )?;
            freed_versions += n;
            // Died-at-victim: also safe to drop — the row was already dead
            // for all queries past the victim, and the victim itself is
            // going away. (Earlier-snapshot visibility of this row IS lost;
            // this is the MVP simplification, acceptable for ephemeral
            // anonymous-class snapshots per DESIGN §6.1.)
            let n = tx.execute(
                &format!("DELETE FROM {table} WHERE visible_until = ?1"),
                params![snap],
            )?;
            freed_versions += n;
        }
    }
    let mut freed_snapshots = 0usize;
    {
        let mut del = tx.prepare("DELETE FROM snapshot WHERE id = ?1")?;
        for snap in &victims {
            freed_snapshots += del.execute(params![snap])?;
        }
    }
    tx.commit()?;

    // Step 4: return pages to the OS.
    let _ = conn.execute_batch("PRAGMA incremental_vacuum");

    let db_size_after = db_path
        .as_deref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0);

    Ok(GcReport {
        freed_snapshots,
        freed_versions,
        db_size_before,
        db_size_after,
    })
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
        conn
    }

    /// Insert a snapshot row at a given epoch second.
    fn insert_snapshot(conn: &Connection, id: i64, kind: &str, created_at: i64) {
        conn.execute(
            "INSERT INTO snapshot (id, uuid, kind, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, format!("uuid-{id}"), kind, created_at],
        )
        .unwrap();
    }

    /// Insert one `symbol_version` row, sharing a single fixture
    /// `symbol_identity` (zero-id) and `file_identity` (zero-id). `born` /
    /// `until` are snapshot ids; if `until` is `Some`, the row is "dead".
    fn insert_version(conn: &Connection, born: i64, until: Option<i64>) {
        let zero16 = [0u8; 16];
        let one16 = {
            let mut b = [0u8; 16];
            b[15] = 1;
            b
        };
        conn.execute(
            "INSERT OR IGNORE INTO file_identity (id, path, language)
             VALUES (?1, '/tmp', 'rust')",
            params![&zero16[..]],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO symbol_identity
                (id, file_identity_id, qualified_name, name, kind)
             VALUES (?1, ?2, 'qn', 'n', 1)",
            params![&one16[..], &zero16[..]],
        )
        .unwrap();
        // Each version_id must be unique; derive a 16-byte BLOB from a counter
        // + nanos so successive inserts collide neither with each other nor with
        // the fixture identity rows.
        use std::sync::atomic::{AtomicU64, Ordering};
        static CTR: AtomicU64 = AtomicU64::new(0);
        let seq = CTR.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let mut vid = [0u8; 16];
        vid[0..8].copy_from_slice(&nanos.to_be_bytes());
        vid[8..16].copy_from_slice(&seq.to_be_bytes());
        conn.execute(
            "INSERT INTO symbol_version
                (id, identity_id, start_byte, end_byte, start_line, end_line,
                 content_hash, visible_from, visible_until)
             VALUES (?1, ?2, 0, 1, 1, 1, 'h', ?3, ?4)",
            params![&vid[..], &one16[..], born, until],
        )
        .unwrap();
    }

    #[test]
    fn gc_removes_old_anonymous_with_dead_versions() {
        let mut conn = db();
        // Old anonymous snapshot, beyond keep window.
        let old_secs = now_secs() - 100 * 86_400;
        insert_snapshot(&conn, 1, "anonymous", old_secs);
        // Recent anonymous snapshot, within keep window.
        insert_snapshot(&conn, 2, "anonymous", now_secs() - 1);
        // A symbol_version born at 1, killed at 2 — fully dead.
        insert_version(&conn, 1, Some(2));

        let policy = GcPolicy {
            keep_anonymous_snapshots: 1, // keep only the most recent
            keep_days: 30,
            ..GcPolicy::default()
        };
        let report = run(&mut conn, &policy).unwrap();
        assert_eq!(report.freed_snapshots, 1, "snapshot 1 should be freed");
        assert_eq!(
            report.freed_versions, 1,
            "the dead version row should be freed"
        );

        // Snapshot 2 still present, snapshot 1 gone.
        let remaining: Vec<i64> = conn
            .prepare("SELECT id FROM snapshot ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(remaining, vec![2]);
    }

    #[test]
    fn gc_preserves_commit_kind_snapshots() {
        let mut conn = db();
        // Old commit snapshot — must NOT be touched.
        insert_snapshot(&conn, 1, "commit", now_secs() - 365 * 86_400);
        // Old filehash snapshot — must NOT be touched (only anonymous/working_tree).
        insert_snapshot(&conn, 2, "filehash", now_secs() - 365 * 86_400);

        let report = run(&mut conn, &GcPolicy::default()).unwrap();
        assert_eq!(report.freed_snapshots, 0);
        assert_eq!(report.freed_versions, 0);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM snapshot", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn gc_respects_keep_anonymous_count() {
        let mut conn = db();
        // Three old anonymous snapshots.
        insert_snapshot(&conn, 1, "anonymous", now_secs() - 100 * 86_400);
        insert_snapshot(&conn, 2, "anonymous", now_secs() - 99 * 86_400);
        insert_snapshot(&conn, 3, "anonymous", now_secs() - 98 * 86_400);

        let policy = GcPolicy {
            keep_anonymous_snapshots: 2, // keep the 2 most recent (ids 2, 3)
            keep_days: 30,
            ..GcPolicy::default()
        };
        let report = run(&mut conn, &policy).unwrap();
        assert_eq!(report.freed_snapshots, 1, "only id=1 should be freed");

        let remaining: Vec<i64> = conn
            .prepare("SELECT id FROM snapshot ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(remaining, vec![2, 3]);
    }

    #[test]
    fn empty_db_is_a_no_op() {
        let mut conn = db();
        let report = run(&mut conn, &GcPolicy::default()).unwrap();
        assert_eq!(report.freed_snapshots, 0);
        assert_eq!(report.freed_versions, 0);
    }
}
