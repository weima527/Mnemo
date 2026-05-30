//! `symbol_usefulness` — materialised per-symbol ranking signal.
//!
//! Each row aggregates how many times a symbol was included in a Context Pack
//! and at what average score. Incrementally maintained by [`bump_inclusion`]
//! from the daemon's `context.find` path; read in bulk by `plan_context`
//! ([`mnemo_index::context`]) to add a soft boost to symbols that have
//! repeatedly proved useful.
//!
//! **Scope:** persistent rows reference `symbol_identity(id)` (FK). Bumps for
//! overlay-only symbols (which never enter `symbol_identity`) will FK-fail; the
//! caller is expected to log-and-skip those — losing some signal on uncommitted
//! edits is acceptable for an MVP feedback loop.

use mnemo_core::{CoreError, SymbolIdentityId};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// One materialised usefulness row. Values are clamped to non-negative `u32`
/// at read time so callers can boost without arithmetic guards.
#[derive(Debug, Clone, Copy, Default)]
pub struct Usefulness {
    /// How many Context Packs have included this symbol.
    pub times_included: u32,
    /// How many of those inclusions were confirmed useful downstream.
    pub times_accepted: u32,
    /// Unix seconds of the most recent inclusion.
    pub last_seen: i64,
    /// Running arithmetic mean of the inclusion scores (0–1000).
    pub avg_score: u32,
}

/// Increment `times_included` for `identity` and update the running-mean
/// `avg_score`. Creates the row on first inclusion. `score` is 0–1000.
///
/// Returns `Err` if the identity is not present in `symbol_identity` (the FK
/// is enforced). Callers in the daemon path log-and-skip such rows.
pub fn bump_inclusion(
    conn: &Connection,
    identity: SymbolIdentityId,
    score: u32,
) -> Result<(), CoreError> {
    let now = now_secs();
    // `identity` is passed via its `ToSql` impl, which stores 16 BLOB bytes —
    // matching how `symbol_identity.id` is written (see `mnemo-core::ids::sqlite_impl`).
    // Writing TEXT here would silently break the FK to symbol_identity.
    //
    // UPSERT: on conflict, blend the score as arithmetic mean *before*
    // incrementing the count. SQLite's UPSERT references the existing row
    // values in unqualified column names, and `excluded.X` references the
    // values being inserted — so this is mean over N+1 samples.
    conn.execute(
        "INSERT INTO symbol_usefulness
            (identity_id, times_included, times_accepted, last_seen, avg_score)
         VALUES (?1, 1, 0, ?2, ?3)
         ON CONFLICT(identity_id) DO UPDATE SET
            last_seen      = excluded.last_seen,
            avg_score      = (avg_score * times_included + excluded.avg_score)
                             / (times_included + 1),
            times_included = times_included + 1",
        params![identity, now, score as i64],
    )?;
    Ok(())
}

/// Load all usefulness rows into an in-memory map. Typically small — only
/// previously-included symbols have rows.
pub fn all(conn: &Connection) -> Result<HashMap<SymbolIdentityId, Usefulness>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT identity_id, times_included, times_accepted, last_seen, avg_score
         FROM symbol_usefulness",
    )?;
    // identity_id is read via the FromSql impl on SymbolIdentityId — matches
    // the BLOB write in `bump_inclusion`.
    let rows = stmt.query_map([], |row| {
        let id: SymbolIdentityId = row.get(0)?;
        let times_included: i64 = row.get(1)?;
        let times_accepted: i64 = row.get(2)?;
        let last_seen: i64 = row.get(3)?;
        let avg_score: i64 = row.get(4)?;
        Ok((
            id,
            Usefulness {
                times_included: times_included.max(0) as u32,
                times_accepted: times_accepted.max(0) as u32,
                last_seen,
                avg_score: avg_score.max(0) as u32,
            },
        ))
    })?;
    let mut out = HashMap::new();
    for r in rows {
        let (id, u) = r?;
        out.insert(id, u);
    }
    Ok(out)
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

    fn db_no_fk() -> Connection {
        // Disable FK so tests can bump usefulness without first inserting a
        // matching symbol_identity row; the bump logic itself is what we test.
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn
    }

    fn id(byte: u8) -> SymbolIdentityId {
        SymbolIdentityId::from_bytes([byte; 16])
    }

    #[test]
    fn bump_creates_then_aggregates_arithmetic_mean() {
        let conn = db_no_fk();
        let a = id(1);
        bump_inclusion(&conn, a, 800).unwrap();
        bump_inclusion(&conn, a, 600).unwrap();
        bump_inclusion(&conn, a, 1000).unwrap();
        let map = all(&conn).unwrap();
        let u = map.get(&a).unwrap();
        assert_eq!(u.times_included, 3);
        // (800 → mean(800,600)=700 → mean(700,700,1000)/3 → integer: 800)
        // After step 3: (700 * 2 + 1000) / 3 = 800.
        assert_eq!(u.avg_score, 800);
    }

    #[test]
    fn distinct_identities_get_distinct_rows() {
        let conn = db_no_fk();
        bump_inclusion(&conn, id(1), 500).unwrap();
        bump_inclusion(&conn, id(2), 700).unwrap();
        let map = all(&conn).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map[&id(1)].times_included, 1);
        assert_eq!(map[&id(1)].avg_score, 500);
        assert_eq!(map[&id(2)].times_included, 1);
        assert_eq!(map[&id(2)].avg_score, 700);
    }

    #[test]
    fn empty_db_yields_empty_map() {
        let conn = db_no_fk();
        assert!(all(&conn).unwrap().is_empty());
    }
}
