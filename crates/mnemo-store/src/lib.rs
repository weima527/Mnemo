//! Embedded SQLite storage layer for Mnemo.
//!
//! Manages schema creation, migrations, and typed queries.
//!
//! DB location: `~/.mnemo/projects/<uuid>/index.db` (per DESIGN §3.1).
//! The target project directory is never modified (zero-pollution contract).

use mnemo_core::CoreError;
use rusqlite::Connection;
use std::path::Path;

pub mod paths;
pub mod registry;
pub mod schema;

/// Open (or create) the Mnemo SQLite database at `path`.
///
/// Applies all DESIGN §2.5 pragmas and runs pending migrations.
pub fn open_database(path: &Path) -> Result<Connection, CoreError> {
    let conn = Connection::open(path)?;

    // DESIGN §2.5 pragmas — ordered for correct interdependency.
    conn.pragma_update(None, "journal_mode", "WAL")?;      // WAL before anything else
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "cache_size", -65536)?;        // 64 MB page cache
    conn.pragma_update(None, "mmap_size", 268435456)?;      // 256 MB mmap
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;        // 5s write lock wait
    conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?; // GC-friendly

    // Run schema initialization / migrations.
    schema::migrate(&conn)?;

    tracing::info!(path = %path.display(), "database opened");
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn open_in_memory_database() {
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
    }

    #[test]
    fn open_database_creates_file() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = open_database(&db_path).unwrap();

        // Verify pragma applied
        let journal: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(journal.to_lowercase(), "wal");

        // Verify schema exists
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(!tables.is_empty());
    }
}
