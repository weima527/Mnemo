//! Project key-value metadata (`meta` table).

use mnemo_core::CoreError;
use rusqlite::{params, Connection, OptionalExtension};

/// Set a metadata key (insert or overwrite).
pub fn set(conn: &Connection, key: &str, value: &str) -> Result<(), CoreError> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Get a metadata value, if present.
pub fn get(conn: &Connection, key: &str) -> Result<Option<String>, CoreError> {
    let value = conn
        .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(value)
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
    fn set_get_roundtrip_and_overwrite() {
        let conn = db();
        assert_eq!(get(&conn, "k").unwrap(), None);
        set(&conn, "k", "v1").unwrap();
        assert_eq!(get(&conn, "k").unwrap().as_deref(), Some("v1"));
        set(&conn, "k", "v2").unwrap();
        assert_eq!(get(&conn, "k").unwrap().as_deref(), Some("v2"));
    }
}
