//! Snapshot rows (`snapshot` table) — point-in-time graph views.

use mnemo_core::{CoreError, SnapshotId};
use rusqlite::{params, Connection};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Process-local sequence. Combined with the wall clock it keeps snapshot
/// `uuid`s unique even when the system clock has coarse resolution (Windows).
static SNAPSHOT_SEQ: AtomicU64 = AtomicU64::new(0);

/// Create a snapshot row and return its auto-increment [`SnapshotId`].
///
/// `kind` is e.g. `"git"` or `"filehash"`. `commit_sha` is set for git
/// snapshots. `parent` links to the previous snapshot, if any.
pub fn create(
    conn: &Connection,
    kind: &str,
    commit_sha: Option<&str>,
    label: Option<&str>,
    parent: Option<SnapshotId>,
) -> Result<SnapshotId, CoreError> {
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let created_at = (now_nanos / 1_000_000_000) as i64;
    conn.execute(
        "INSERT INTO snapshot (uuid, kind, commit_sha, label, parent_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![external_uuid(now_nanos), kind, commit_sha, label, parent, created_at],
    )?;
    Ok(SnapshotId(conn.last_insert_rowid()))
}

/// The most recent snapshot id, if any.
pub fn latest(conn: &Connection) -> Result<Option<SnapshotId>, CoreError> {
    let id: Option<i64> = conn.query_row("SELECT MAX(id) FROM snapshot", [], |row| row.get(0))?;
    Ok(id.map(SnapshotId))
}

/// Total number of snapshots.
pub fn count(conn: &Connection) -> Result<i64, CoreError> {
    let n = conn.query_row("SELECT COUNT(*) FROM snapshot", [], |row| row.get(0))?;
    Ok(n)
}

/// A stable external label for a snapshot.
///
/// The snapshot's *logical* id is the autoincrement [`SnapshotId`]; this uuid
/// is only an external reference and is derived (not random) from the creation
/// time plus a process-local sequence — so it is unique without relying on
/// `Uuid::new_v4()`.
fn external_uuid(now_nanos: u128) -> String {
    let seq = SNAPSHOT_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{now_nanos:032x}{seq:016x}")
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

    #[test]
    fn create_increments_and_latest_tracks() {
        let conn = db();
        assert_eq!(latest(&conn).unwrap(), None);
        let s1 = create(&conn, "manual", None, None, None).unwrap();
        let s2 = create(&conn, "manual", None, None, Some(s1)).unwrap();
        assert!(i64::from(s2) > i64::from(s1));
        assert_eq!(latest(&conn).unwrap(), Some(s2));
        assert_eq!(count(&conn).unwrap(), 2);
    }
}
