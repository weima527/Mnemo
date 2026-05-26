//! Symbol identities and versions (`symbol_identity` / `symbol_version`).

use mnemo_core::{
    CoreError, FileIdentityId, Range, SnapshotId, SymbolIdentityId, SymbolKind, SymbolVersionId,
};
use rusqlite::{params, Connection, OptionalExtension};

/// A symbol visible at a snapshot, joined across identity + version.
///
/// Note: `symbol_version` stores byte/line spans but not columns, so callers
/// reconstructing a [`Range`] use column `0` placeholders.
#[derive(Debug, Clone)]
pub struct SymbolRecord {
    /// Stable identity (survives re-index).
    pub identity_id: SymbolIdentityId,
    /// Version visible at the queried snapshot.
    pub version_id: SymbolVersionId,
    /// File the symbol is defined in.
    pub file_identity_id: FileIdentityId,
    /// In-file qualified name.
    pub qualified_name: String,
    /// Bare name.
    pub name: String,
    /// Symbol kind.
    pub kind: SymbolKind,
    /// Start byte offset of the definition.
    pub start_byte: usize,
    /// End byte offset of the definition.
    pub end_byte: usize,
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based end line.
    pub end_line: u32,
}

/// Insert a symbol identity if absent (idempotent on `id`).
pub fn upsert_identity(
    conn: &Connection,
    id: SymbolIdentityId,
    file_identity_id: FileIdentityId,
    qualified_name: &str,
    name: &str,
    kind: SymbolKind,
) -> Result<(), CoreError> {
    conn.execute(
        "INSERT INTO symbol_identity (id, file_identity_id, qualified_name, name, kind)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
            file_identity_id = excluded.file_identity_id,
            qualified_name   = excluded.qualified_name,
            name             = excluded.name,
            kind             = excluded.kind",
        params![id, file_identity_id, qualified_name, name, kind.to_db()],
    )?;
    Ok(())
}

/// Whether an *open* version of `identity` already has this `content_hash`.
///
/// Used for incremental indexing: an unchanged symbol must not produce a new
/// version row.
pub fn open_version_matches(
    conn: &Connection,
    identity: SymbolIdentityId,
    content_hash: &str,
) -> Result<bool, CoreError> {
    let hit: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM symbol_version
             WHERE identity_id = ?1 AND content_hash = ?2 AND visible_until IS NULL
             LIMIT 1",
            params![identity, content_hash],
            |row| row.get(0),
        )
        .optional()?;
    Ok(hit.is_some())
}

/// Insert a new open symbol version (`visible_until = NULL`) at `visible_from`.
pub fn insert_version(
    conn: &Connection,
    version_id: SymbolVersionId,
    identity_id: SymbolIdentityId,
    range: &Range,
    content_hash: &str,
    visible_from: SnapshotId,
) -> Result<(), CoreError> {
    conn.execute(
        "INSERT INTO symbol_version
            (id, identity_id, start_byte, end_byte, start_line, end_line, content_hash,
             visible_from, visible_until)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
        params![
            version_id,
            identity_id,
            range.start_byte as i64,
            range.end_byte as i64,
            range.start_line as i64,
            range.end_line as i64,
            content_hash,
            visible_from,
        ],
    )?;
    Ok(())
}

/// Close every open version of `identity` as of `visible_until`.
///
/// Returns the number of versions closed.
pub fn close_open_versions(
    conn: &Connection,
    identity: SymbolIdentityId,
    visible_until: SnapshotId,
) -> Result<usize, CoreError> {
    let closed = conn.execute(
        "UPDATE symbol_version SET visible_until = ?1
         WHERE identity_id = ?2 AND visible_until IS NULL",
        params![visible_until, identity],
    )?;
    Ok(closed)
}

/// Every symbol visible at `snapshot`.
pub fn at_snapshot(
    conn: &Connection,
    snapshot: SnapshotId,
) -> Result<Vec<SymbolRecord>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT si.id, sv.id, si.file_identity_id, si.qualified_name, si.name, si.kind,
                sv.start_byte, sv.end_byte, sv.start_line, sv.end_line
         FROM symbol_version sv
         JOIN symbol_identity si ON si.id = sv.identity_id
         WHERE sv.visible_from <= ?1 AND (sv.visible_until IS NULL OR ?1 < sv.visible_until)",
    )?;
    let rows = stmt.query_map(params![snapshot], row_to_record)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Symbols with an exact `name` visible at `snapshot`.
pub fn by_name(
    conn: &Connection,
    name: &str,
    snapshot: SnapshotId,
) -> Result<Vec<SymbolRecord>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT si.id, sv.id, si.file_identity_id, si.qualified_name, si.name, si.kind,
                sv.start_byte, sv.end_byte, sv.start_line, sv.end_line
         FROM symbol_version sv
         JOIN symbol_identity si ON si.id = sv.identity_id
         WHERE si.name = ?1
           AND sv.visible_from <= ?2 AND (sv.visible_until IS NULL OR ?2 < sv.visible_until)",
    )?;
    let rows = stmt.query_map(params![name, snapshot], row_to_record)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Map a joined `symbol_identity`/`symbol_version` row to a [`SymbolRecord`].
fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SymbolRecord> {
    Ok(SymbolRecord {
        identity_id: row.get(0)?,
        version_id: row.get(1)?,
        file_identity_id: row.get(2)?,
        qualified_name: row.get(3)?,
        name: row.get(4)?,
        kind: SymbolKind::from_db(row.get::<_, i64>(5)?),
        start_byte: row.get::<_, i64>(6)? as usize,
        end_byte: row.get::<_, i64>(7)? as usize,
        start_line: row.get::<_, i64>(8)? as u32,
        end_line: row.get::<_, i64>(9)? as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dao::{file, snapshot};
    use crate::schema;
    use mnemo_core::ProjectId;
    use std::path::Path;

    fn range() -> Range {
        Range {
            start_byte: 0,
            end_byte: 10,
            start_line: 1,
            end_line: 2,
            start_column: 1,
            end_column: 1,
        }
    }

    #[test]
    fn insert_and_query_at_snapshot() {
        // PLAN M1.3 exit test: insert at S1, close at S2, query both snapshots.
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();

        let proj = ProjectId::from_canonical_path(Path::new("/tmp/p"));
        let file_id = FileIdentityId::derive(proj, "src/lib.rs");
        file::upsert_identity(&conn, file_id, "src/lib.rs", "rust").unwrap();

        let ident = SymbolIdentityId::derive(proj, "src/lib.rs", "foo", SymbolKind::Function);
        upsert_identity(&conn, ident, file_id, "foo", "foo", SymbolKind::Function).unwrap();

        // S1: insert an open version.
        let s1 = snapshot::create(&conn, "manual", None, None, None).unwrap();
        let ver = SymbolVersionId::from_bytes([7u8; 16]);
        insert_version(&conn, ver, ident, &range(), "hash-v1", s1).unwrap();

        let at_s1 = at_snapshot(&conn, s1).unwrap();
        assert_eq!(at_s1.len(), 1);
        assert_eq!(at_s1[0].qualified_name, "foo");

        // S2: close the version.
        let s2 = snapshot::create(&conn, "manual", None, None, Some(s1)).unwrap();
        assert_eq!(close_open_versions(&conn, ident, s2).unwrap(), 1);

        // Visible at S1, gone at S2 (time-travel).
        assert_eq!(at_snapshot(&conn, s1).unwrap().len(), 1);
        assert_eq!(at_snapshot(&conn, s2).unwrap().len(), 0);
    }

    #[test]
    fn open_version_matches_detects_unchanged() {
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
        let proj = ProjectId::from_canonical_path(Path::new("/tmp/p"));
        let file_id = FileIdentityId::derive(proj, "src/lib.rs");
        file::upsert_identity(&conn, file_id, "src/lib.rs", "rust").unwrap();
        let ident = SymbolIdentityId::derive(proj, "src/lib.rs", "foo", SymbolKind::Function);
        upsert_identity(&conn, ident, file_id, "foo", "foo", SymbolKind::Function).unwrap();

        let s1 = snapshot::create(&conn, "manual", None, None, None).unwrap();
        insert_version(&conn, SymbolVersionId::from_bytes([1u8; 16]), ident, &range(), "h1", s1)
            .unwrap();

        assert!(open_version_matches(&conn, ident, "h1").unwrap());
        assert!(!open_version_matches(&conn, ident, "h2").unwrap());
    }

    #[test]
    fn by_name_filters_to_visible() {
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
        let proj = ProjectId::from_canonical_path(Path::new("/tmp/p"));
        let file_id = FileIdentityId::derive(proj, "src/lib.rs");
        file::upsert_identity(&conn, file_id, "src/lib.rs", "rust").unwrap();
        let ident = SymbolIdentityId::derive(proj, "src/lib.rs", "foo", SymbolKind::Function);
        upsert_identity(&conn, ident, file_id, "foo", "foo", SymbolKind::Function).unwrap();
        let s1 = snapshot::create(&conn, "manual", None, None, None).unwrap();
        insert_version(&conn, SymbolVersionId::from_bytes([2u8; 16]), ident, &range(), "h1", s1)
            .unwrap();

        assert_eq!(by_name(&conn, "foo", s1).unwrap().len(), 1);
        assert_eq!(by_name(&conn, "missing", s1).unwrap().len(), 0);
    }
}
