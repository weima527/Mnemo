//! Telemetry event log (`telemetry_event` table).
//!
//! Append-only audit trail of tool calls and ranking decisions. Used downstream
//! to materialise [`super::usefulness`] (and, eventually, to back replay /
//! debugging). Each row carries an integer `event_kind` (see [`kind`]) and an
//! opaque JSON `payload`.

use mnemo_core::CoreError;
use rusqlite::{params, Connection};
use std::time::{SystemTime, UNIX_EPOCH};

/// Stable integer codes per DESIGN §2.4 convention: only-add-never-reuse.
pub mod kind {
    /// A Context Pack was built and returned to a caller.
    pub const CONTEXT_PACK_BUILT: i64 = 1;
    /// A specific symbol was included in a Context Pack.
    pub const SYMBOL_INCLUDED: i64 = 2;
    /// A previously included symbol was confirmed useful by a downstream signal.
    pub const SYMBOL_ACCEPTED: i64 = 3;
    /// A tool was invoked (catch-all observability).
    pub const TOOL_INVOKED: i64 = 4;
}

/// Append one event row. `payload` is opaque JSON (or empty).
pub fn append(conn: &Connection, event_kind: i64, payload: &str) -> Result<(), CoreError> {
    let created_at = now_secs();
    conn.execute(
        "INSERT INTO telemetry_event (event_kind, payload, created_at) VALUES (?1, ?2, ?3)",
        params![event_kind, payload, created_at],
    )?;
    Ok(())
}

/// Total number of rows with `event_kind`. Used by tests and lightweight
/// reporting; production aggregation goes through [`super::usefulness`].
pub fn count_by_kind(conn: &Connection, event_kind: i64) -> Result<i64, CoreError> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM telemetry_event WHERE event_kind = ?1",
        params![event_kind],
        |row| row.get(0),
    )?;
    Ok(n)
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

    #[test]
    fn append_and_count_per_kind() {
        let conn = db();
        assert_eq!(count_by_kind(&conn, kind::SYMBOL_INCLUDED).unwrap(), 0);
        append(&conn, kind::SYMBOL_INCLUDED, r#"{"id":"abc"}"#).unwrap();
        append(&conn, kind::SYMBOL_INCLUDED, r#"{"id":"def"}"#).unwrap();
        append(&conn, kind::CONTEXT_PACK_BUILT, r#"{"items":2}"#).unwrap();
        assert_eq!(count_by_kind(&conn, kind::SYMBOL_INCLUDED).unwrap(), 2);
        assert_eq!(count_by_kind(&conn, kind::CONTEXT_PACK_BUILT).unwrap(), 1);
        assert_eq!(count_by_kind(&conn, kind::SYMBOL_ACCEPTED).unwrap(), 0);
    }
}
