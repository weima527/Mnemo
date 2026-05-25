//! Database schema and migration logic.
//!
//! Uses a simple version-table approach:
//! - `schema_version` tracks the current version.
//! - Each migration is an idempotent function that bumps the version.

use mnemo_core::CoreError;
use rusqlite::Connection;

/// Current schema version. Increment this when adding migrations.
const LATEST_VERSION: u32 = 1;

/// Ensure the database is at the latest schema version.
pub fn migrate(conn: &Connection) -> Result<(), CoreError> {
    // Create the version table if it doesn't exist.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY
        );",
    )?;

    // Read current version (default 0 if no row).
    let current: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    for v in (current + 1)..=LATEST_VERSION {
        apply_migration(conn, v)?;
        tracing::info!(version = v, "migration applied");
    }

    Ok(())
}

/// Create the initial (v1) schema.
fn apply_migration(conn: &Connection, version: u32) -> Result<(), CoreError> {
    match version {
        1 => migration_v1(conn),
        _ => Err(CoreError::migration_error(
            version,
            version,
            "unknown migration version",
        )),
    }
}

/// V1: core tables — repos, files, symbols, edges, snapshots, overlays.
fn migration_v1(conn: &Connection) -> Result<(), CoreError> {
    conn.execute_batch(
        "
        -- A repository being indexed.
        CREATE TABLE IF NOT EXISTS repo (
            id          TEXT PRIMARY KEY,
            path        TEXT NOT NULL UNIQUE,
            name        TEXT NOT NULL,
            language    TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- A file within a repository.
        CREATE TABLE IF NOT EXISTS file (
            id          TEXT PRIMARY KEY,
            repo_id     TEXT NOT NULL REFERENCES repo(id) ON DELETE CASCADE,
            path        TEXT NOT NULL,
            language    TEXT,
            hash        TEXT NOT NULL,
            size_bytes  INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(repo_id, path)
        );

        -- A symbol extracted from source code.
        CREATE TABLE IF NOT EXISTS symbol (
            id              TEXT PRIMARY KEY,
            file_id         TEXT NOT NULL REFERENCES file(id) ON DELETE CASCADE,
            name            TEXT NOT NULL,
            qualified_name  TEXT,
            kind            TEXT NOT NULL,
            start_byte      INTEGER NOT NULL,
            end_byte        INTEGER NOT NULL,
            start_line      INTEGER NOT NULL,
            end_line        INTEGER NOT NULL,
            start_column    INTEGER NOT NULL,
            end_column      INTEGER NOT NULL,
            confidence      REAL NOT NULL DEFAULT 1.0,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(file_id, name, start_line, start_column)
        );

        -- A directed edge between two symbols.
        CREATE TABLE IF NOT EXISTS edge (
            id          TEXT PRIMARY KEY,
            from_id     TEXT NOT NULL REFERENCES symbol(id) ON DELETE CASCADE,
            to_id       TEXT NOT NULL REFERENCES symbol(id) ON DELETE CASCADE,
            kind        TEXT NOT NULL,
            file_id     TEXT REFERENCES file(id),
            start_byte  INTEGER,
            end_byte    INTEGER,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(from_id, to_id, kind)
        );

        -- An immutable snapshot of the graph at a point in time.
        CREATE TABLE IF NOT EXISTS snapshot (
            id          TEXT PRIMARY KEY,
            repo_id     TEXT NOT NULL REFERENCES repo(id) ON DELETE CASCADE,
            label       TEXT,
            commit_sha  TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Which files belong to a snapshot (materialized for fast lookup).
        CREATE TABLE IF NOT EXISTS snapshot_file (
            snapshot_id TEXT NOT NULL REFERENCES snapshot(id) ON DELETE CASCADE,
            file_id     TEXT NOT NULL REFERENCES file(id) ON DELETE CASCADE,
            PRIMARY KEY (snapshot_id, file_id)
        );

        -- A working-tree or PR overlay (delta on top of a base snapshot).
        CREATE TABLE IF NOT EXISTS overlay (
            id              TEXT PRIMARY KEY,
            base_snapshot_id TEXT NOT NULL REFERENCES snapshot(id) ON DELETE CASCADE,
            label           TEXT,
            kind            TEXT NOT NULL DEFAULT 'working_tree',
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Changed files within an overlay (add / modify / delete).
        CREATE TABLE IF NOT EXISTS overlay_file (
            overlay_id  TEXT NOT NULL REFERENCES overlay(id) ON DELETE CASCADE,
            file_id     TEXT NOT NULL REFERENCES file(id) ON DELETE CASCADE,
            change_kind TEXT NOT NULL DEFAULT 'modified',
            PRIMARY KEY (overlay_id, file_id)
        );

        -- A generated Context Pack.
        CREATE TABLE IF NOT EXISTS context_pack (
            id          TEXT PRIMARY KEY,
            repo_id     TEXT NOT NULL REFERENCES repo(id) ON DELETE CASCADE,
            task        TEXT NOT NULL,
            token_budget INTEGER,
            estimated_tokens INTEGER,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Items included in a Context Pack.
        CREATE TABLE IF NOT EXISTS context_pack_item (
            id              TEXT PRIMARY KEY,
            pack_id         TEXT NOT NULL REFERENCES context_pack(id) ON DELETE CASCADE,
            item_kind       TEXT NOT NULL,
            symbol_id       TEXT REFERENCES symbol(id),
            file_id         TEXT REFERENCES file(id),
            reason          TEXT,
            score           REAL,
            token_estimate  INTEGER
        );

        -- Memory facts (project constitution, preferences, outcomes).
        CREATE TABLE IF NOT EXISTS memory_fact (
            id          TEXT PRIMARY KEY,
            repo_id     TEXT,
            kind        TEXT NOT NULL,
            fact        TEXT NOT NULL,
            confidence  REAL NOT NULL DEFAULT 1.0,
            provenance  TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Telemetry events for savings tracking and outcome signals.
        CREATE TABLE IF NOT EXISTS telemetry_event (
            id          TEXT PRIMARY KEY,
            repo_id     TEXT,
            event_kind  TEXT NOT NULL,
            payload     TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Record the migration version.
        INSERT INTO schema_version (version) VALUES (1);
        ",
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn migration_v1_creates_tables() {
        let conn = Connection::open_in_memory().unwrap();
        migration_v1(&conn).unwrap();

        // Verify key tables exist.
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"repo".to_string()));
        assert!(tables.contains(&"file".to_string()));
        assert!(tables.contains(&"symbol".to_string()));
        assert!(tables.contains(&"edge".to_string()));
        assert!(tables.contains(&"snapshot".to_string()));
        assert!(tables.contains(&"overlay".to_string()));
        assert!(tables.contains(&"context_pack".to_string()));
        assert!(tables.contains(&"memory_fact".to_string()));
    }
}
