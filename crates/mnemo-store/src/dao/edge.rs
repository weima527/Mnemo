//! Edge versions (`edge_version` table).

use mnemo_core::{CoreError, EdgeKind, SnapshotId, SymbolIdentityId};
use rusqlite::{params, Connection, OptionalExtension};

/// Insert an open edge version at `visible_from`.
///
/// `INSERT OR IGNORE` because the primary key is
/// `(from, to, kind, visible_from)`: re-inserting the same edge within one
/// snapshot is a no-op rather than an error.
pub fn insert_version(
    conn: &Connection,
    from: SymbolIdentityId,
    to: SymbolIdentityId,
    kind: EdgeKind,
    confidence: i64,
    visible_from: SnapshotId,
) -> Result<(), CoreError> {
    conn.execute(
        "INSERT OR IGNORE INTO edge_version
            (from_symbol_identity, to_symbol_identity, kind, confidence, visible_from, visible_until)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
        params![from, to, kind.to_db(), confidence, visible_from],
    )?;
    Ok(())
}

/// Whether an *open* edge `(from, to, kind)` already exists (visible_until
/// NULL). Used for incremental indexing so an unchanged edge is not re-inserted
/// under a new snapshot.
pub fn open_exists(
    conn: &Connection,
    from: SymbolIdentityId,
    to: SymbolIdentityId,
    kind: EdgeKind,
) -> Result<bool, CoreError> {
    let hit: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM edge_version
             WHERE from_symbol_identity = ?1 AND to_symbol_identity = ?2 AND kind = ?3
               AND visible_until IS NULL
             LIMIT 1",
            params![from, to, kind.to_db()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(hit.is_some())
}

/// Identity ids of symbols that `Calls` `target` (incoming edges) at `snapshot`.
pub fn callers_of(
    conn: &Connection,
    target: SymbolIdentityId,
    snapshot: SnapshotId,
) -> Result<Vec<SymbolIdentityId>, CoreError> {
    neighbors(
        conn,
        "SELECT from_symbol_identity FROM edge_version
         WHERE to_symbol_identity = ?1 AND kind = ?2
           AND visible_from <= ?3 AND (visible_until IS NULL OR ?3 < visible_until)",
        target,
        snapshot,
    )
}

/// Identity ids of symbols `target` `Calls` (outgoing edges) at `snapshot`.
pub fn callees_of(
    conn: &Connection,
    source: SymbolIdentityId,
    snapshot: SnapshotId,
) -> Result<Vec<SymbolIdentityId>, CoreError> {
    neighbors(
        conn,
        "SELECT to_symbol_identity FROM edge_version
         WHERE from_symbol_identity = ?1 AND kind = ?2
           AND visible_from <= ?3 AND (visible_until IS NULL OR ?3 < visible_until)",
        source,
        snapshot,
    )
}

/// Shared body for `callers_of` / `callees_of` (Calls edges only).
fn neighbors(
    conn: &Connection,
    sql: &str,
    pivot: SymbolIdentityId,
    snapshot: SnapshotId,
) -> Result<Vec<SymbolIdentityId>, CoreError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![pivot, EdgeKind::Calls.to_db(), snapshot], |row| {
        row.get(0)
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Every edge visible at `snapshot`, as `(from, to, kind)`.
pub fn at_snapshot(
    conn: &Connection,
    snapshot: SnapshotId,
) -> Result<Vec<(SymbolIdentityId, SymbolIdentityId, EdgeKind)>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT from_symbol_identity, to_symbol_identity, kind FROM edge_version
         WHERE visible_from <= ?1 AND (visible_until IS NULL OR ?1 < visible_until)",
    )?;
    let rows = stmt.query_map(params![snapshot], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            EdgeKind::from_db(row.get::<_, i64>(2)?),
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dao::{file, snapshot, symbol};
    use crate::schema;
    use mnemo_core::{ProjectId, Range, SymbolKind, SymbolVersionId};
    use std::path::Path;

    fn setup() -> (Connection, ProjectId, SnapshotId) {
        let conn = Connection::open_in_memory().unwrap();
        schema::migrate(&conn).unwrap();
        let proj = ProjectId::from_canonical_path(Path::new("/tmp/p"));
        let s1 = snapshot::create(&conn, "manual", None, None, None).unwrap();
        (conn, proj, s1)
    }

    fn define(
        conn: &Connection,
        proj: ProjectId,
        qname: &str,
        snapshot: SnapshotId,
        version_byte: u8,
    ) -> SymbolIdentityId {
        let file_id = mnemo_core::FileIdentityId::derive(proj, "src/lib.rs");
        file::upsert_identity(conn, file_id, "src/lib.rs", "rust").unwrap();
        let ident = SymbolIdentityId::derive(proj, "src/lib.rs", qname, SymbolKind::Function);
        symbol::upsert_identity(conn, ident, file_id, qname, qname, SymbolKind::Function).unwrap();
        let range = Range {
            start_byte: 0,
            end_byte: 1,
            start_line: 1,
            end_line: 1,
            start_column: 1,
            end_column: 1,
        };
        symbol::insert_version(
            conn,
            SymbolVersionId::from_bytes([version_byte; 16]),
            ident,
            &range,
            "h",
            snapshot,
        )
        .unwrap();
        ident
    }

    #[test]
    fn callers_and_callees_roundtrip() {
        let (conn, proj, s1) = setup();
        let foo = define(&conn, proj, "foo", s1, 1);
        let bar = define(&conn, proj, "bar", s1, 2);

        insert_version(&conn, foo, bar, EdgeKind::Calls, 100, s1).unwrap();
        // Duplicate insert is ignored by the composite PK.
        insert_version(&conn, foo, bar, EdgeKind::Calls, 100, s1).unwrap();

        assert_eq!(callees_of(&conn, foo, s1).unwrap(), vec![bar]);
        assert_eq!(callers_of(&conn, bar, s1).unwrap(), vec![foo]);
        assert_eq!(at_snapshot(&conn, s1).unwrap().len(), 1);
    }
}
