//! Symbol graph construction, storage, and traversal.
//!
//! The graph is the structural backbone of Mnemo. It links symbols
//! through edges (calls, imports, contains, implements, file-depends)
//! and supports bounded traversal for context planning and impact analysis.

use mnemo_core::{Edge, EdgeKind, FileId, SnapshotId, Symbol, SymbolId};

/// An in-memory working view of the code graph for a snapshot.
///
/// This is a flat adjacency representation — not a full graph database.
/// For large repos it can be backed by the store crate's SQLite tables
/// with lazy loading.
#[derive(Debug, Default)]
pub struct SymbolGraph {
    /// All symbols in this snapshot.
    symbols: Vec<Symbol>,
    /// All edges in this snapshot.
    edges: Vec<Edge>,
}

impl SymbolGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a symbol to the graph.
    pub fn add_symbol(&mut self, symbol: Symbol) {
        self.symbols.push(symbol);
    }

    /// Add an edge to the graph.
    pub fn add_edge(&mut self, edge: Edge) {
        self.edges.push(edge);
    }

    /// Find a symbol by its id.
    pub fn find_symbol(&self, id: SymbolId) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.id == id)
    }

    /// Find all symbols in a given file.
    pub fn symbols_in_file(&self, file_id: FileId) -> Vec<&Symbol> {
        self.symbols
            .iter()
            .filter(|s| s.file_id == file_id)
            .collect()
    }

    /// Return all callers of a symbol (incoming Calls edges).
    pub fn callers_of(&self, target: SymbolId) -> Vec<&Symbol> {
        self.edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls && e.to == target)
            .filter_map(|e| self.find_symbol(e.from))
            .collect()
    }

    /// Return all callees of a symbol (outgoing Calls edges).
    pub fn callees_of(&self, source: SymbolId) -> Vec<&Symbol> {
        self.edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls && e.from == source)
            .filter_map(|e| self.find_symbol(e.to))
            .collect()
    }

    /// Return all edges of a given kind.
    pub fn edges_of_kind(&self, kind: EdgeKind) -> Vec<&Edge> {
        self.edges.iter().filter(|e| e.kind == kind).collect()
    }

    /// Number of symbols in the graph.
    pub fn symbol_count(&self) -> usize {
        self.symbols.len()
    }

    /// Number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mnemo_core::{Language, Range};

    fn make_symbol(id: SymbolId, name: &str) -> Symbol {
        Symbol {
            id,
            name: name.to_string(),
            qualified_name: None,
            kind: mnemo_core::SymbolKind::Function,
            file_id: FileId::new_v4(),
            definition_range: Range {
                start_byte: 0,
                end_byte: 10,
                start_line: 1,
                end_line: 1,
                start_column: 1,
                end_column: 10,
            },
            language: Language::Rust,
        }
    }

    #[test]
    fn add_and_find_symbol() {
        let mut g = SymbolGraph::new();
        let id = SymbolId::new_v4();
        let sym = make_symbol(id, "main");
        g.add_symbol(sym.clone());
        assert_eq!(g.find_symbol(id).unwrap().name, "main");
    }

    #[test]
    fn caller_callee_traversal() {
        let mut g = SymbolGraph::new();
        let a = SymbolId::new_v4();
        let b = SymbolId::new_v4();

        g.add_symbol(make_symbol(a, "foo"));
        g.add_symbol(make_symbol(b, "bar"));

        g.add_edge(Edge {
            from: a,
            to: b,
            kind: EdgeKind::Calls,
            location: None,
        });

        assert_eq!(g.callers_of(b).len(), 1);
        assert_eq!(g.callers_of(b)[0].name, "foo");
        assert_eq!(g.callees_of(a).len(), 1);
        assert_eq!(g.callees_of(a)[0].name, "bar");
    }
}
