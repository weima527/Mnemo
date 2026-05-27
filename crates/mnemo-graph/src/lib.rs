//! In-memory symbol graph for a single snapshot.
//!
//! A flat adjacency view built from data the store crate reads at a snapshot
//! (the index crate does the loading; this crate stays free of any DB
//! dependency). It indexes symbols by id and name and supports caller/callee
//! lookup plus bounded BFS for impact analysis.

use mnemo_core::{EdgeKind, FileIdentityId, SymbolIdentityId, SymbolKind};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// A display-ready symbol node in the hydrated graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    /// Stable identity of the symbol.
    pub identity: SymbolIdentityId,
    /// Bare name (e.g. `parse_file`).
    pub name: String,
    /// In-file qualified name (e.g. `Point::manhattan`).
    pub qualified_name: String,
    /// Symbol kind.
    pub kind: SymbolKind,
    /// File the symbol is defined in.
    pub file_id: FileIdentityId,
    /// Repo-relative path of the defining file (for display).
    pub file_path: String,
    /// 1-based start line of the definition.
    pub start_line: u32,
    /// 1-based end line of the definition (inclusive).
    pub end_line: u32,
    /// Byte length of the definition's source (for token estimation).
    pub byte_len: usize,
}

/// A flat adjacency view of the code graph at one snapshot.
#[derive(Debug, Default)]
pub struct SymbolGraph {
    nodes: HashMap<SymbolIdentityId, GraphNode>,
    /// `from -> [callees]` for `Calls` edges.
    out_calls: HashMap<SymbolIdentityId, Vec<SymbolIdentityId>>,
    /// `to -> [callers]` for `Calls` edges.
    in_calls: HashMap<SymbolIdentityId, Vec<SymbolIdentityId>>,
    /// name -> identities (a name may be defined more than once).
    by_name: HashMap<String, Vec<SymbolIdentityId>>,
}

impl SymbolGraph {
    /// Build a graph from its nodes and `(from, to, kind)` edges.
    ///
    /// Only `Calls` edges populate the caller/callee adjacency for the MVP.
    pub fn build(
        nodes: Vec<GraphNode>,
        edges: &[(SymbolIdentityId, SymbolIdentityId, EdgeKind)],
    ) -> Self {
        let mut graph = SymbolGraph::default();
        for node in nodes {
            graph
                .by_name
                .entry(node.name.clone())
                .or_default()
                .push(node.identity);
            graph.nodes.insert(node.identity, node);
        }
        for &(from, to, kind) in edges {
            if kind == EdgeKind::Calls {
                graph.out_calls.entry(from).or_default().push(to);
                graph.in_calls.entry(to).or_default().push(from);
            }
        }
        graph
    }

    /// Look up a node by identity.
    pub fn node(&self, id: SymbolIdentityId) -> Option<&GraphNode> {
        self.nodes.get(&id)
    }

    /// Iterate every node in the graph (unordered).
    pub fn iter_nodes(&self) -> impl Iterator<Item = &GraphNode> {
        self.nodes.values()
    }

    /// Identities of every symbol with the given bare name.
    pub fn find_by_name(&self, name: &str) -> &[SymbolIdentityId] {
        self.by_name.get(name).map_or(&[], Vec::as_slice)
    }

    /// Direct callers of `id` (incoming `Calls` edges).
    pub fn callers_of(&self, id: SymbolIdentityId) -> &[SymbolIdentityId] {
        self.in_calls.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Direct callees of `id` (outgoing `Calls` edges).
    pub fn callees_of(&self, id: SymbolIdentityId) -> &[SymbolIdentityId] {
        self.out_calls.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Nodes whose name or qualified name contains `needle` (case-insensitive).
    pub fn nodes_matching(&self, needle: &str) -> Vec<&GraphNode> {
        let needle = needle.to_lowercase();
        self.nodes
            .values()
            .filter(|n| {
                n.name.to_lowercase().contains(&needle)
                    || n.qualified_name.to_lowercase().contains(&needle)
            })
            .collect()
    }

    /// Number of symbols.
    pub fn symbol_count(&self) -> usize {
        self.nodes.len()
    }

    /// Number of `Calls` edges.
    pub fn edge_count(&self) -> usize {
        self.out_calls.values().map(Vec::len).sum()
    }

    /// Transitive callers of `start` up to `max_depth`, as `(id, depth)`.
    ///
    /// Depth 0 (the start node) is not included in the output; each reachable
    /// caller is reported once, at its shortest depth.
    pub fn bfs_callers(
        &self,
        start: SymbolIdentityId,
        max_depth: u32,
    ) -> Vec<(SymbolIdentityId, u32)> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        seen.insert(start);
        queue.push_back((start, 0u32));
        while let Some((id, depth)) = queue.pop_front() {
            if depth > 0 {
                out.push((id, depth));
            }
            if depth >= max_depth {
                continue;
            }
            for &caller in self.callers_of(id) {
                if seen.insert(caller) {
                    queue.push_back((caller, depth + 1));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(byte: u8, name: &str) -> GraphNode {
        GraphNode {
            identity: SymbolIdentityId::from_bytes([byte; 16]),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            file_id: FileIdentityId::ZERO,
            file_path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 1,
            byte_len: 0,
        }
    }

    fn calls(from: u8, to: u8) -> (SymbolIdentityId, SymbolIdentityId, EdgeKind) {
        (
            SymbolIdentityId::from_bytes([from; 16]),
            SymbolIdentityId::from_bytes([to; 16]),
            EdgeKind::Calls,
        )
    }

    #[test]
    fn build_and_traverse() {
        // foo -> bar, baz -> bar
        let graph = SymbolGraph::build(
            vec![node(1, "foo"), node(2, "bar"), node(3, "baz")],
            &[calls(1, 2), calls(3, 2)],
        );
        let bar = SymbolIdentityId::from_bytes([2; 16]);
        let foo = SymbolIdentityId::from_bytes([1; 16]);

        assert_eq!(graph.symbol_count(), 3);
        assert_eq!(graph.edge_count(), 2);
        assert_eq!(graph.find_by_name("foo"), &[foo]);
        assert_eq!(graph.callers_of(bar).len(), 2);
        assert_eq!(graph.callees_of(foo), &[bar]);
        assert!(graph.callees_of(bar).is_empty());
    }

    #[test]
    fn bfs_callers_respects_depth() {
        // a -> b -> c  (c is called by b, b by a)
        let graph = SymbolGraph::build(
            vec![node(1, "a"), node(2, "b"), node(3, "c")],
            &[calls(1, 2), calls(2, 3)],
        );
        let c = SymbolIdentityId::from_bytes([3; 16]);
        let b = SymbolIdentityId::from_bytes([2; 16]);
        let a = SymbolIdentityId::from_bytes([1; 16]);

        let depth1 = graph.bfs_callers(c, 1);
        assert_eq!(depth1, vec![(b, 1)]);

        let depth2 = graph.bfs_callers(c, 2);
        assert_eq!(depth2, vec![(b, 1), (a, 2)]);
    }

    #[test]
    fn nodes_matching_is_case_insensitive() {
        let graph = SymbolGraph::build(vec![node(1, "parse_file"), node(2, "other")], &[]);
        let hits = graph.nodes_matching("PARSE");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "parse_file");
    }
}
