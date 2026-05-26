//! Global project registry (`~/.mnemo/registry.db`).
//!
//! Per DESIGN §3.3, the registry tracks all projects known to this
//! Mnemo instance. Each row maps a canonical project path to its
//! stable `ProjectId` and runtime metadata.

use mnemo_core::{CoreError, ProjectId};
use rusqlite::{params, Connection};

/// Open (or create) the global registry database.
pub fn open_registry() -> Result<Connection, CoreError> {
    let path = super::paths::registry_db_path();
    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(CoreError::Io)?;
    }

    let conn = Connection::open(&path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?; // tolerate concurrent attach/index

    // Initialize schema.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS project (
            uuid             TEXT PRIMARY KEY,
            canonical_path   TEXT NOT NULL UNIQUE,
            display_name     TEXT NOT NULL,
            snapshot_source  TEXT NOT NULL DEFAULT 'unknown',
            created_at       INTEGER NOT NULL,
            last_attached_at INTEGER NOT NULL,
            last_indexed_at  INTEGER,
            db_size_bytes    INTEGER,
            status           TEXT NOT NULL DEFAULT 'active'
        );
        CREATE INDEX IF NOT EXISTS idx_project_path ON project(canonical_path);",
    )?;

    Ok(conn)
}

/// Information about a registered project.
#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub uuid: ProjectId,
    pub canonical_path: String,
    pub display_name: String,
    pub snapshot_source: String,
    pub status: String,
    pub last_attached_at: i64,
    pub last_indexed_at: Option<i64>,
    pub db_size_bytes: Option<i64>,
}

/// Register or update a project in the registry.
pub fn upsert_project(
    conn: &Connection,
    uuid: ProjectId,
    canonical_path: &str,
    display_name: &str,
) -> Result<(), CoreError> {
    let now = unix_now();
    conn.execute(
        "INSERT INTO project (uuid, canonical_path, display_name, snapshot_source, created_at, last_attached_at, status)
         VALUES (?1, ?2, ?3, 'unknown', ?4, ?4, 'active')
         ON CONFLICT(uuid) DO UPDATE SET
           canonical_path = excluded.canonical_path,
           display_name = excluded.display_name,
           last_attached_at = excluded.last_attached_at,
           status = 'active'",
        params![
            uuid.to_hex(),
            canonical_path,
            display_name,
            now,
        ],
    )?;

    Ok(())
}

/// Mark a project's indexing timestamp and update DB size.
pub fn update_index_stats(
    conn: &Connection,
    uuid: ProjectId,
    db_size_bytes: i64,
) -> Result<(), CoreError> {
    let now = unix_now();
    conn.execute(
        "UPDATE project SET last_indexed_at = ?1, db_size_bytes = ?2 WHERE uuid = ?3",
        params![now, db_size_bytes, uuid.to_hex()],
    )?;
    Ok(())
}

/// List all active projects.
pub fn list_active_projects(conn: &Connection) -> Result<Vec<ProjectEntry>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT uuid, canonical_path, display_name, snapshot_source, status,
                last_attached_at, last_indexed_at, db_size_bytes
         FROM project WHERE status = 'active' ORDER BY last_attached_at DESC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(ProjectEntry {
            uuid: row.get::<_, String>(0)?.parse().unwrap_or(ProjectId::ZERO),
            canonical_path: row.get(1)?,
            display_name: row.get(2)?,
            snapshot_source: row.get(3)?,
            status: row.get(4)?,
            last_attached_at: row.get(5)?,
            last_indexed_at: row.get(6)?,
            db_size_bytes: row.get(7)?,
        })
    })?;

    let mut projects = Vec::new();
    for row in rows {
        projects.push(row?);
    }
    Ok(projects)
}

/// Mark a project as archived (detach without deleting DB).
pub fn archive_project(conn: &Connection, uuid: ProjectId) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE project SET status = 'archived' WHERE uuid = ?1",
        params![uuid.to_hex()],
    )?;
    Ok(())
}

/// Return current unix timestamp (seconds).
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_registry_creates_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS project (
                uuid TEXT PRIMARY KEY, canonical_path TEXT NOT NULL UNIQUE,
                display_name TEXT NOT NULL, snapshot_source TEXT NOT NULL DEFAULT 'unknown',
                created_at INTEGER NOT NULL, last_attached_at INTEGER NOT NULL,
                last_indexed_at INTEGER, db_size_bytes INTEGER,
                status TEXT NOT NULL DEFAULT 'active'
            );",
        ).unwrap();

        upsert_project(&conn, ProjectId::ZERO, "/tmp/test", "test-project").unwrap();

        let projects = list_active_projects(&conn).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].display_name, "test-project");
    }
}
