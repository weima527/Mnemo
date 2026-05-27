//! Build an in-memory graph that overlays uncommitted working-tree edits on the
//! base snapshot — **without writing the DB** (DESIGN §6.3, iron-law §12 #7).
//!
//! The cached `SymbolGraph` keeps only `Calls` edges and can't be iterated, so
//! the overlaid graph is rebuilt from the base symbols/edges read from the DB
//! (read-only) at the snapshot, with the changed files re-parsed in memory and
//! their edges re-resolved against the merged symbol set.

use mnemo_core::{
    CoreError, EdgeKind, FileIdentityId, ProjectId, RawEdge, SnapshotId, SymbolIdentityId,
};
use mnemo_graph::{GraphNode, SymbolGraph};
use mnemo_parser::parse_file;
use mnemo_resolve::{resolve_file_edges, ResolvedSymbol, SymbolIndex};
use mnemo_store::dao::{edge as edge_dao, file as file_dao, symbol as symbol_dao};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// One changed working-tree file. `content == None` means the file was deleted.
#[derive(Debug, Clone)]
pub struct OverlayFile {
    /// Repo-relative path (forward slashes), e.g. `src/math.rs`.
    pub rel_path: String,
    /// New file contents, or `None` if the file was deleted.
    pub content: Option<String>,
}

/// Build a [`SymbolGraph`] for the base `snapshot` with the `overlay` files'
/// edits applied in memory. Reads the DB but never writes it.
pub fn hydrate_with_overlay(
    conn: &Connection,
    snapshot: SnapshotId,
    project_id: ProjectId,
    overlay: &[OverlayFile],
) -> Result<SymbolGraph, CoreError> {
    // The set of changed/deleted files, by identity.
    let changed: HashMap<FileIdentityId, &OverlayFile> = overlay
        .iter()
        .map(|f| (FileIdentityId::derive(project_id, &f.rel_path), f))
        .collect();

    let file_paths: HashMap<FileIdentityId, String> = file_dao::paths(conn)?.into_iter().collect();
    let base_symbols = symbol_dao::at_snapshot(conn, snapshot)?;
    let base_edges = edge_dao::at_snapshot(conn, snapshot)?;

    // Which file each base symbol belongs to (used to filter base edges).
    let base_id_file: HashMap<SymbolIdentityId, FileIdentityId> = base_symbols
        .iter()
        .map(|s| (s.identity_id, s.file_identity_id))
        .collect();

    // Merged nodes: base nodes from unchanged files ...
    let mut nodes: Vec<GraphNode> = Vec::new();
    for s in &base_symbols {
        if changed.contains_key(&s.file_identity_id) {
            continue;
        }
        nodes.push(GraphNode {
            identity: s.identity_id,
            name: s.name.clone(),
            qualified_name: s.qualified_name.clone(),
            kind: s.kind,
            file_id: s.file_identity_id,
            file_path: file_paths
                .get(&s.file_identity_id)
                .cloned()
                .unwrap_or_default(),
            start_line: s.start_line,
            end_line: s.end_line,
            byte_len: s.end_byte.saturating_sub(s.start_byte),
        });
    }

    // ... plus freshly parsed symbols from the (non-deleted) changed files.
    let mut overlay_raw: Vec<(FileIdentityId, Vec<RawEdge>)> = Vec::new();
    for file in overlay {
        let Some(content) = &file.content else {
            continue; // deleted file contributes no symbols
        };
        let file_id = FileIdentityId::derive(project_id, &file.rel_path);
        let parsed = parse_file(file_id, Path::new(&file.rel_path), content);
        let mut seen: HashSet<SymbolIdentityId> = HashSet::new();
        for raw in parsed.symbols {
            let identity =
                SymbolIdentityId::derive(project_id, &file.rel_path, &raw.qualified_name, raw.kind);
            if !seen.insert(identity) {
                continue;
            }
            nodes.push(GraphNode {
                identity,
                name: raw.name,
                qualified_name: raw.qualified_name,
                kind: raw.kind,
                file_id,
                file_path: file.rel_path.clone(),
                start_line: raw.definition_range.start_line,
                end_line: raw.definition_range.end_line,
                byte_len: raw
                    .definition_range
                    .end_byte
                    .saturating_sub(raw.definition_range.start_byte),
            });
        }
        overlay_raw.push((file_id, parsed.edges));
    }

    // Resolution index over the merged symbol set (base ∪ overlay).
    let index = SymbolIndex::build(
        nodes
            .iter()
            .map(|n| ResolvedSymbol {
                identity: n.identity,
                file: n.file_id,
                name: n.name.clone(),
                qualified_name: n.qualified_name.clone(),
            })
            .collect(),
    );

    // Merged edges: base edges originating in unchanged files ...
    let mut edges: Vec<(SymbolIdentityId, SymbolIdentityId, EdgeKind)> = Vec::new();
    for (from, to, kind) in base_edges {
        let originates_in_changed = base_id_file
            .get(&from)
            .is_some_and(|f| changed.contains_key(f));
        if !originates_in_changed {
            edges.push((from, to, kind));
        }
    }
    // ... plus the changed files' edges re-resolved against the merged index.
    for (file_id, raw_edges) in &overlay_raw {
        let (resolved, _unresolved) = resolve_file_edges(&index, *file_id, raw_edges);
        for edge in resolved {
            edges.push((edge.from, edge.to, edge.kind));
        }
    }

    Ok(SymbolGraph::build(nodes, &edges))
}
