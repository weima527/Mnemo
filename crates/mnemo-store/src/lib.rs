//! Embedded SQLite storage layer for Mnemo.
//!
//! Manages schema creation, migrations, and typed queries for
//! repositories, files, symbols, edges, snapshots, overlays,
//! Context Packs, and memory facts.

use mnemo_core::CoreError;
use rusqlite::Connection;
use std::path::Path;

pub mod schema;

/// Open (or create) the Mnemo SQLite database at `path`.
///
/// Runs pending migrations automatically.
pub fn open_database(path: &Path) -> Result<Connection, CoreError> {
    let conn = Connection::open(path)?;

    // Enable WAL mode for better concurrent-read performance.
    conn.pragma_update(None, "journal_mode", "WAL")?;

    // Enable foreign key enforcement.
    conn.pragma_update(None, "foreign_keys", "ON")?;

    // Run schema initialization / migrations.
    schema::migrate(&conn)?;

    tracing::info!(path = %path.display(), "database opened");
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_database() {
        // rusqlite special path — in-memory DB
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
    }
}
