//! High-level read queries over an indexed project.
//!
//! Each entry point resolves the project, opens its DB, hydrates a
//! [`SymbolGraph`] at the latest snapshot, and answers a single question. The
//! `rusqlite::Connection` never escapes this module, so callers (the CLI) stay
//! free of any storage dependency.
//!
//! A project that has never been indexed (no snapshot) yields empty results
//! rather than an error.

use mnemo_core::{CoreError, SnapshotId, SymbolKind};
use mnemo_graph::{GraphNode, SymbolGraph};
use mnemo_store::dao::{
    edge as edge_dao, file as file_dao, snapshot as snapshot_dao, symbol as symbol_dao,
};
use mnemo_store::open_database;
use mnemo_store::paths::{project_db, resolve_project_id};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// A symbol rendered for query output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolHit {
    /// In-file qualified name (e.g. `Point::manhattan`).
    pub qualified_name: String,
    /// Symbol kind.
    pub kind: SymbolKind,
    /// Repo-relative path of the defining file.
    pub file_path: String,
    /// 1-based start line of the definition.
    pub start_line: u32,
}

impl SymbolHit {
    fn from_node(node: &GraphNode) -> Self {
        Self {
            qualified_name: node.qualified_name.clone(),
            kind: node.kind,
            file_path: node.file_path.clone(),
            start_line: node.start_line,
        }
    }
}

/// A matched symbol together with its related symbols (callers or callees).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relations {
    /// The symbol that matched the queried name.
    pub symbol: SymbolHit,
    /// Its callers or callees, depending on the query.
    pub related: Vec<SymbolHit>,
}

// ---------------------------------------------------------------------------
// Path-based queries (CLI): open the project DB, hydrate, then delegate to the
// graph-based fns below. Return empty when the project was never indexed.
// ---------------------------------------------------------------------------

/// Symbols that call any symbol named `name`.
pub fn callers(repo_path: &Path, name: &str) -> Result<Vec<Relations>, CoreError> {
    Ok(match open_graph(repo_path)? {
        Some(graph) => callers_in(&graph, name),
        None => Vec::new(),
    })
}

/// Symbols called by any symbol named `name`.
pub fn callees(repo_path: &Path, name: &str) -> Result<Vec<Relations>, CoreError> {
    Ok(match open_graph(repo_path)? {
        Some(graph) => callees_in(&graph, name),
        None => Vec::new(),
    })
}

/// Symbols whose name or qualified name contains `pattern` (case-insensitive).
pub fn search(repo_path: &Path, pattern: &str) -> Result<Vec<SymbolHit>, CoreError> {
    Ok(match open_graph(repo_path)? {
        Some(graph) => search_in(&graph, pattern),
        None => Vec::new(),
    })
}

/// Details for every symbol named `name`.
pub fn symbol_info(repo_path: &Path, name: &str) -> Result<Vec<SymbolHit>, CoreError> {
    Ok(match open_graph(repo_path)? {
        Some(graph) => symbols_named(&graph, name),
        None => Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Graph-based queries: operate on an already-hydrated graph. The daemon (M2.2)
// calls these on each project's cached `SymbolGraph`, avoiding a per-request DB
// hydrate.
// ---------------------------------------------------------------------------

/// Callers of any symbol named `name`, within `graph`.
pub fn callers_in(graph: &SymbolGraph, name: &str) -> Vec<Relations> {
    relations(graph, name, Direction::Callers)
}

/// Callees of any symbol named `name`, within `graph`.
pub fn callees_in(graph: &SymbolGraph, name: &str) -> Vec<Relations> {
    relations(graph, name, Direction::Callees)
}

/// Symbols in `graph` whose name or qualified name contains `pattern`
/// (case-insensitive), sorted by qualified name.
pub fn search_in(graph: &SymbolGraph, pattern: &str) -> Vec<SymbolHit> {
    let mut hits: Vec<SymbolHit> = graph
        .nodes_matching(pattern)
        .into_iter()
        .map(SymbolHit::from_node)
        .collect();
    hits.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    hits
}

/// Details for every symbol named `name` in `graph`.
pub fn symbols_named(graph: &SymbolGraph, name: &str) -> Vec<SymbolHit> {
    graph
        .find_by_name(name)
        .iter()
        .filter_map(|&id| graph.node(id))
        .map(SymbolHit::from_node)
        .collect()
}

enum Direction {
    Callers,
    Callees,
}

fn relations(graph: &SymbolGraph, name: &str, dir: Direction) -> Vec<Relations> {
    graph
        .find_by_name(name)
        .iter()
        .filter_map(|&id| graph.node(id).map(|node| (id, node)))
        .map(|(id, node)| {
            let related_ids = match dir {
                Direction::Callers => graph.callers_of(id),
                Direction::Callees => graph.callees_of(id),
            };
            let related = related_ids
                .iter()
                .filter_map(|&r| graph.node(r))
                .map(SymbolHit::from_node)
                .collect();
            Relations {
                symbol: SymbolHit::from_node(node),
                related,
            }
        })
        .collect()
}

/// Resolve the project, open its DB, and hydrate the graph at the latest
/// snapshot. Returns `None` when the project has never been indexed.
fn open_graph(repo_path: &Path) -> Result<Option<SymbolGraph>, CoreError> {
    let (project_id, _canonical) = resolve_project_id(repo_path)?;
    let conn = open_database(&project_db(project_id))?;
    let Some(snapshot) = snapshot_dao::latest(&conn)? else {
        return Ok(None);
    };
    Ok(Some(hydrate(&conn, snapshot)?))
}

/// Hydrate an in-memory [`SymbolGraph`] from the DB at `snapshot`.
///
/// Public so the daemon's tenant layer (M2.1) can build a `ProjectContext`'s
/// graph cache from a connection it already holds.
pub fn hydrate(conn: &Connection, snapshot: SnapshotId) -> Result<SymbolGraph, CoreError> {
    let file_paths: HashMap<_, _> = file_dao::paths(conn)?.into_iter().collect();
    let nodes = symbol_dao::at_snapshot(conn, snapshot)?
        .into_iter()
        .map(|s| GraphNode {
            identity: s.identity_id,
            name: s.name,
            qualified_name: s.qualified_name,
            kind: s.kind,
            file_id: s.file_identity_id,
            file_path: file_paths
                .get(&s.file_identity_id)
                .cloned()
                .unwrap_or_default(),
            start_line: s.start_line,
        })
        .collect::<Vec<_>>();
    let edges = edge_dao::at_snapshot(conn, snapshot)?;
    Ok(SymbolGraph::build(nodes, &edges))
}
