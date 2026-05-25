//! Database schema and migration logic.
//!
//! Per DESIGN §2.3: identity/version split with MVCC visibility intervals.
//!
//! Core principle: every fact is versioned through `(visible_from, visible_until)`.
//! The logical identity of a symbol (its name, file, kind) is stable and stored in
//! `symbol_identity`. The physical location and content of each version is stored
//! in `symbol_version`. This separation enables:
//!
//! - Incremental indexing: unchanged symbols produce no new rows.
//! - Time-travel queries: query the graph at any past snapshot.
//! - GC: prune old snapshots without losing identity continuity.

use mnemo_core::CoreError;
use rusqlite::Connection;

/// Current schema version. Increment this when adding migrations.
const LATEST_VERSION: u32 = 1;

/// Ensure the database is at the latest schema version.
pub fn migrate(conn: &Connection) -> Result<(), CoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY
        );",
    )?;

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

/// V1: identity/version split per DESIGN §2.3 — 13 physical + 1 virtual table.
fn migration_v1(conn: &Connection) -> Result<(), CoreError> {
    conn.execute_batch(
        "
        -- Project-level metadata (key-value)
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        -- Snapshot: point-in-time immutable graph view
        CREATE TABLE IF NOT EXISTS snapshot (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            uuid        TEXT NOT NULL UNIQUE,
            kind        TEXT NOT NULL,
            commit_sha  TEXT,
            label       TEXT,
            parent_id   INTEGER REFERENCES snapshot(id),
            created_at  INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_snapshot_kind_created
            ON snapshot(kind, created_at);

        -- File identity (stable across versions)
        CREATE TABLE IF NOT EXISTS file_identity (
            id         TEXT PRIMARY KEY,
            path       TEXT NOT NULL UNIQUE,
            language   TEXT NOT NULL
        );

        -- File version (physical state at a snapshot)
        CREATE TABLE IF NOT EXISTS file_version (
            id            TEXT PRIMARY KEY,
            identity_id   TEXT NOT NULL REFERENCES file_identity(id),
            content_hash  TEXT NOT NULL,
            size_bytes    INTEGER NOT NULL,
            visible_from  INTEGER NOT NULL REFERENCES snapshot(id),
            visible_until INTEGER REFERENCES snapshot(id)
        );
        CREATE INDEX IF NOT EXISTS idx_file_version_visible
            ON file_version(identity_id, visible_from, visible_until);

        -- Symbol identity (stable across versions)
        CREATE TABLE IF NOT EXISTS symbol_identity (
            id              TEXT PRIMARY KEY,
            file_identity_id TEXT NOT NULL REFERENCES file_identity(id),
            qualified_name  TEXT NOT NULL,
            name            TEXT NOT NULL,
            kind            INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_symbol_identity_name
            ON symbol_identity(name);

        -- Symbol version (physical state at a snapshot)
        CREATE TABLE IF NOT EXISTS symbol_version (
            id              TEXT PRIMARY KEY,
            identity_id     TEXT NOT NULL REFERENCES symbol_identity(id),
            start_byte      INTEGER NOT NULL,
            end_byte        INTEGER NOT NULL,
            start_line      INTEGER NOT NULL,
            end_line        INTEGER NOT NULL,
            content_hash    TEXT NOT NULL,
            visible_from    INTEGER NOT NULL REFERENCES snapshot(id),
            visible_until   INTEGER REFERENCES snapshot(id)
        );
        CREATE INDEX IF NOT EXISTS idx_symbol_version_visible
            ON symbol_version(identity_id, visible_from, visible_until);

        -- Edge version (WITHOUT ROWID, composite PK)
        CREATE TABLE IF NOT EXISTS edge_version (
            from_symbol_identity TEXT NOT NULL REFERENCES symbol_identity(id),
            to_symbol_identity   TEXT NOT NULL REFERENCES symbol_identity(id),
            kind                 INTEGER NOT NULL,
            confidence           INTEGER NOT NULL,
            visible_from         INTEGER NOT NULL REFERENCES snapshot(id),
            visible_until        INTEGER REFERENCES snapshot(id),
            PRIMARY KEY (from_symbol_identity, to_symbol_identity, kind, visible_from)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS idx_edge_to
            ON edge_version(to_symbol_identity, kind, visible_from, visible_until);

        -- FTS5 full-text search on symbol names
        CREATE VIRTUAL TABLE IF NOT EXISTS symbol_fts USING fts5(
            qualified_name,
            name,
            content='symbol_identity',
            content_rowid='rowid'
        );
        CREATE TRIGGER IF NOT EXISTS symbol_identity_ai AFTER INSERT ON symbol_identity BEGIN
            INSERT INTO symbol_fts(rowid, qualified_name, name)
            VALUES (new.rowid, new.qualified_name, new.name);
        END;
        CREATE TRIGGER IF NOT EXISTS symbol_identity_au AFTER UPDATE ON symbol_identity BEGIN
            INSERT INTO symbol_fts(symbol_fts, rowid, qualified_name, name)
            VALUES ('delete', old.rowid, old.qualified_name, old.name);
            INSERT INTO symbol_fts(rowid, qualified_name, name)
            VALUES (new.rowid, new.qualified_name, new.name);
        END;
        CREATE TRIGGER IF NOT EXISTS symbol_identity_ad AFTER DELETE ON symbol_identity BEGIN
            INSERT INTO symbol_fts(symbol_fts, rowid, qualified_name, name)
            VALUES ('delete', old.rowid, old.qualified_name, old.name);
        END;

        -- Context Pack
        CREATE TABLE IF NOT EXISTS context_pack (
            id                TEXT PRIMARY KEY,
            snapshot_id       INTEGER NOT NULL REFERENCES snapshot(id),
            task              TEXT NOT NULL,
            token_budget      INTEGER NOT NULL,
            estimated_tokens  INTEGER NOT NULL,
            created_at        INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS context_pack_item (
            pack_id              TEXT NOT NULL REFERENCES context_pack(id) ON DELETE CASCADE,
            rank                 INTEGER NOT NULL,
            symbol_identity_id   TEXT REFERENCES symbol_identity(id),
            file_identity_id     TEXT REFERENCES file_identity(id),
            reason               TEXT NOT NULL,
            score                INTEGER NOT NULL,
            PRIMARY KEY (pack_id, rank)
        ) WITHOUT ROWID;

        -- Memory facts (constitution, preferences, outcomes)
        CREATE TABLE IF NOT EXISTS memory_fact (
            id          TEXT PRIMARY KEY,
            kind        TEXT NOT NULL,
            promotion   TEXT NOT NULL DEFAULT 'raw',
            body        TEXT NOT NULL,
            confidence  INTEGER NOT NULL,
            provenance  TEXT NOT NULL,
            created_at  INTEGER NOT NULL,
            updated_at  INTEGER NOT NULL
        );

        -- Telemetry events
        CREATE TABLE IF NOT EXISTS telemetry_event (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            event_kind  INTEGER NOT NULL,
            payload     TEXT,
            created_at  INTEGER NOT NULL
        );

        -- Materialized symbol usefulness for ranking
        CREATE TABLE IF NOT EXISTS symbol_usefulness (
            identity_id      TEXT PRIMARY KEY REFERENCES symbol_identity(id),
            times_included   INTEGER NOT NULL DEFAULT 0,
            times_accepted   INTEGER NOT NULL DEFAULT 0,
            last_seen        INTEGER NOT NULL,
            avg_score        INTEGER NOT NULL DEFAULT 0
        );

        -- Record migration
        INSERT INTO schema_version (version) VALUES (1);
        ",
    )?;
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn migration_v1_creates_all_tables() {
        let conn = Connection::open_in_memory().unwrap();
        migration_v1(&conn).unwrap();

        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type IN ('table', 'view')
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        for expected in &[
            "meta", "snapshot", "file_identity", "file_version",
            "symbol_identity", "symbol_version", "symbol_fts",
            "edge_version", "context_pack", "context_pack_item",
            "memory_fact", "telemetry_event", "symbol_usefulness",
            "schema_version",
        ] {
            assert!(
                tables.contains(&(*expected).to_string()),
                "missing table: {}",
                expected
            );
        }
    }

    #[test]
    fn edge_version_has_without_rowid() {
        let conn = Connection::open_in_memory().unwrap();
        migration_v1(&conn).unwrap();

        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='edge_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(sql.to_uppercase().contains("WITHOUT ROWID"));
    }

    #[test]
    fn context_pack_item_has_without_rowid() {
        let conn = Connection::open_in_memory().unwrap();
        migration_v1(&conn).unwrap();

        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='context_pack_item'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(sql.to_uppercase().contains("WITHOUT ROWID"));
    }

    #[test]
    fn fts5_triggers_created() {
        let conn = Connection::open_in_memory().unwrap();
        migration_v1(&conn).unwrap();

        let triggers: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='trigger' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(triggers.iter().any(|t| t == "symbol_identity_ai"));
        assert!(triggers.iter().any(|t| t == "symbol_identity_au"));
        assert!(triggers.iter().any(|t| t == "symbol_identity_ad"));
    }

    #[test]
    fn edge_version_composite_pk_allows_same_edge_multiple_snapshots() {
        let conn = Connection::open_in_memory().unwrap();
        migration_v1(&conn).unwrap();

        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('project_uuid', 'test')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO snapshot (id, uuid, kind, created_at) VALUES (1, 's1', 'commit', 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO snapshot (id, uuid, kind, created_at) VALUES (2, 's2', 'commit', 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO file_identity (id, path, language) VALUES ('f1', 'src/a.rs', 'rust')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO symbol_identity (id, file_identity_id, qualified_name, name, kind) VALUES ('sa', 'f1', 'a', 'a', 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO symbol_identity (id, file_identity_id, qualified_name, name, kind) VALUES ('sb', 'f1', 'b', 'b', 1)",
            [],
        ).unwrap();

        // Same edge, two snapshots → OK.
        conn.execute(
            "INSERT INTO edge_version (from_symbol_identity, to_symbol_identity, kind, confidence, visible_from)
             VALUES ('sa', 'sb', 1, 100, 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO edge_version (from_symbol_identity, to_symbol_identity, kind, confidence, visible_from)
             VALUES ('sa', 'sb', 1, 100, 2)",
            [],
        ).unwrap();

        let count: i64 = conn
            .query_row("SELECT count(*) FROM edge_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
