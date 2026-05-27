//! File identities (`file_identity` table).
//!
//! File *version* rows (incremental file-level change detection) are deferred;
//! the MVP pipeline detects unchanged work at the symbol-version level. The
//! identity row is still required because `symbol_identity.file_identity_id`
//! references it under `foreign_keys = ON`.

use mnemo_core::{CoreError, FileIdentityId};
use rusqlite::{params, Connection};

/// Insert a file identity if absent (idempotent on `id`).
pub fn upsert_identity(
    conn: &Connection,
    id: FileIdentityId,
    path: &str,
    language: &str,
) -> Result<(), CoreError> {
    conn.execute(
        "INSERT INTO file_identity (id, path, language) VALUES (?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET path = excluded.path, language = excluded.language",
        params![id, path, language],
    )?;
    Ok(())
}

/// All `(file_identity_id, repo-relative path)` pairs. The set is small (one
/// row per file), so loading it whole to build a lookup map is cheap.
pub fn paths(conn: &Connection) -> Result<Vec<(FileIdentityId, String)>, CoreError> {
    let mut stmt = conn.prepare("SELECT id, path FROM file_identity")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use mnemo_core::ProjectId;
    use std::path::Path;

    #[test]
    fn upsert_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
        let proj = ProjectId::from_canonical_path(Path::new("/tmp/p"));
        let id = FileIdentityId::derive(proj, "src/lib.rs");

        upsert_identity(&conn, id, "src/lib.rs", "rust").unwrap();
        upsert_identity(&conn, id, "src/lib.rs", "rust").unwrap();

        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM file_identity", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }
}
