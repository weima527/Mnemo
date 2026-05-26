//! Reference resolution: turn the parser's [`RawEdge`]s into concrete [`Edge`]s.
//!
//! The parser emits references as raw names (`square` calls `mul`). This crate
//! resolves each name to a concrete [`SymbolIdentityId`] using a project-wide
//! symbol index.
//!
//! MVP strategy (DESIGN-aligned, PLAN §3 M1.2), in priority order:
//! 1. a symbol with that name defined in the **same file** wins;
//! 2. otherwise, a **unique** project-wide match wins;
//! 3. multiple project-wide matches → [`UnresolvedReason::Ambiguous`];
//! 4. no match → [`UnresolvedReason::NotFound`].
//!
//! Full trait resolution, `use`-alias handling, and macro expansion are out of
//! scope for the MVP — unresolved references are recorded, not guessed.

use mnemo_core::{Edge, EdgeKind, FileIdentityId, RawEdge, SourceRange, SymbolIdentityId};
use std::collections::{HashMap, HashSet};

/// A defined symbol, carrying the fields the resolver needs to match against.
#[derive(Debug, Clone)]
pub struct ResolvedSymbol {
    /// Stable identity of the symbol.
    pub identity: SymbolIdentityId,
    /// File the symbol is defined in.
    pub file: FileIdentityId,
    /// Bare name (e.g. `add`).
    pub name: String,
    /// In-file qualified name (e.g. `Point::manhattan`).
    pub qualified_name: String,
}

/// Why a reference could not be resolved to a single target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedReason {
    /// No symbol with that name exists anywhere in the project.
    NotFound,
    /// More than one project-wide symbol matched and none was in-file.
    Ambiguous,
}

/// A reference the resolver could not turn into a concrete edge.
#[derive(Debug, Clone)]
pub struct UnresolvedRef {
    /// The symbol the reference originates from.
    pub from: SymbolIdentityId,
    /// The raw name that could not be resolved.
    pub to_name: String,
    /// The kind of reference (e.g. `Calls`).
    pub kind: EdgeKind,
    /// Why resolution failed.
    pub reason: UnresolvedReason,
}

/// An index over all project symbols supporting name and `(file, qname)` lookup.
#[derive(Debug, Default)]
pub struct SymbolIndex {
    symbols: Vec<ResolvedSymbol>,
    /// name → indices into `symbols`.
    by_name: HashMap<String, Vec<usize>>,
    /// (file, qualified_name) → index into `symbols`.
    by_file_qname: HashMap<(FileIdentityId, String), usize>,
}

impl SymbolIndex {
    /// Build an index from every known symbol in the project.
    pub fn build(symbols: Vec<ResolvedSymbol>) -> Self {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_file_qname = HashMap::new();
        for (i, s) in symbols.iter().enumerate() {
            by_name.entry(s.name.clone()).or_default().push(i);
            by_file_qname.insert((s.file, s.qualified_name.clone()), i);
        }
        Self {
            symbols,
            by_name,
            by_file_qname,
        }
    }

    /// Find the originating symbol of a reference: the symbol in `file` whose
    /// qualified name matches.
    fn find_origin(&self, file: FileIdentityId, qualified_name: &str) -> Option<&ResolvedSymbol> {
        self.by_file_qname
            .get(&(file, qualified_name.to_string()))
            .map(|&i| &self.symbols[i])
    }

    /// Resolve a referenced `name` as seen from `file`.
    fn resolve_target(
        &self,
        file: FileIdentityId,
        name: &str,
    ) -> Result<SymbolIdentityId, UnresolvedReason> {
        let candidates = self.by_name.get(name).ok_or(UnresolvedReason::NotFound)?;

        // (1) Same-file definition wins (first in build order).
        if let Some(&i) = candidates.iter().find(|&&i| self.symbols[i].file == file) {
            return Ok(self.symbols[i].identity);
        }
        // (2) Unique project-wide match, else ambiguous.
        match candidates.as_slice() {
            [only] => Ok(self.symbols[*only].identity),
            _ => Err(UnresolvedReason::Ambiguous),
        }
    }
}

/// Resolve the raw edges originating in `file` into concrete edges.
///
/// Returns the resolved edges (deduplicated by `(from, to, kind)`, since the
/// `edge_version` primary key forbids duplicates within a snapshot) and the
/// references that could not be resolved.
pub fn resolve_file_edges(
    index: &SymbolIndex,
    file: FileIdentityId,
    raw_edges: &[RawEdge],
) -> (Vec<Edge>, Vec<UnresolvedRef>) {
    let mut edges = Vec::new();
    let mut unresolved = Vec::new();
    let mut seen: HashSet<(SymbolIdentityId, SymbolIdentityId, EdgeKind)> = HashSet::new();

    for raw in raw_edges {
        // The origin must be a symbol defined in this file; if not, the edge is
        // malformed for this pass and is skipped.
        let Some(origin) = index.find_origin(file, &raw.from_qualified_name) else {
            continue;
        };
        let from = origin.identity;

        match index.resolve_target(file, &raw.to_name) {
            Ok(to) => {
                if seen.insert((from, to, raw.kind)) {
                    edges.push(Edge {
                        from,
                        to,
                        kind: raw.kind,
                        location: raw.location.map(|range| SourceRange { file_id: file, range }),
                    });
                }
            }
            Err(reason) => unresolved.push(UnresolvedRef {
                from,
                to_name: raw.to_name.clone(),
                kind: raw.kind,
                reason,
            }),
        }
    }

    (edges, unresolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mnemo_core::{ProjectId, SymbolKind};
    use std::path::Path;

    fn proj() -> ProjectId {
        ProjectId::from_canonical_path(Path::new("/tmp/x"))
    }

    fn fid(path: &str) -> FileIdentityId {
        FileIdentityId::derive(proj(), path)
    }

    fn sym(file: FileIdentityId, file_path: &str, qname: &str, name: &str) -> ResolvedSymbol {
        ResolvedSymbol {
            identity: SymbolIdentityId::derive(proj(), file_path, qname, SymbolKind::Function),
            file,
            name: name.to_string(),
            qualified_name: qname.to_string(),
        }
    }

    fn call(from: &str, to: &str) -> RawEdge {
        RawEdge {
            from_qualified_name: from.to_string(),
            to_name: to.to_string(),
            kind: EdgeKind::Calls,
            location: None,
        }
    }

    #[test]
    fn same_file_call_resolved() {
        // PLAN M1.2 exit test: foo() calls bar(), both in one file.
        let f = fid("src/math.rs");
        let foo = sym(f, "src/math.rs", "foo", "foo");
        let bar = sym(f, "src/math.rs", "bar", "bar");
        let index = SymbolIndex::build(vec![foo.clone(), bar.clone()]);

        let (edges, unresolved) = resolve_file_edges(&index, f, &[call("foo", "bar")]);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, foo.identity);
        assert_eq!(edges[0].to, bar.identity);
        assert_eq!(edges[0].kind, EdgeKind::Calls);
        assert!(unresolved.is_empty());
    }

    #[test]
    fn cross_file_unique_resolved() {
        // manhattan (lib.rs) calls add, uniquely defined in math.rs.
        let lib = fid("src/lib.rs");
        let math = fid("src/math.rs");
        let manhattan = sym(lib, "src/lib.rs", "Point::manhattan", "manhattan");
        let add = sym(math, "src/math.rs", "add", "add");
        let index = SymbolIndex::build(vec![manhattan, add.clone()]);

        let (edges, unresolved) = resolve_file_edges(&index, lib, &[call("Point::manhattan", "add")]);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, add.identity);
        assert!(unresolved.is_empty());
    }

    #[test]
    fn same_file_beats_global() {
        // A local `add` shadows another `add` defined elsewhere.
        let here = fid("src/here.rs");
        let other = fid("src/other.rs");
        let local_add = sym(here, "src/here.rs", "add", "add");
        let remote_add = sym(other, "src/other.rs", "add", "add");
        let caller = sym(here, "src/here.rs", "caller", "caller");
        let index = SymbolIndex::build(vec![remote_add, local_add.clone(), caller]);

        let (edges, _) = resolve_file_edges(&index, here, &[call("caller", "add")]);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, local_add.identity, "same-file definition must win");
    }

    #[test]
    fn ambiguous_when_multiple_global_and_none_in_file() {
        let a = fid("src/a.rs");
        let b = fid("src/b.rs");
        let caller_file = fid("src/c.rs");
        let index = SymbolIndex::build(vec![
            sym(a, "src/a.rs", "add", "add"),
            sym(b, "src/b.rs", "add", "add"),
            sym(caller_file, "src/c.rs", "use_it", "use_it"),
        ]);

        let (edges, unresolved) = resolve_file_edges(&index, caller_file, &[call("use_it", "add")]);

        assert!(edges.is_empty());
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].reason, UnresolvedReason::Ambiguous);
    }

    #[test]
    fn not_found_when_no_match() {
        let f = fid("src/a.rs");
        let index = SymbolIndex::build(vec![sym(f, "src/a.rs", "caller", "caller")]);

        let (edges, unresolved) = resolve_file_edges(&index, f, &[call("caller", "ghost")]);

        assert!(edges.is_empty());
        assert_eq!(unresolved[0].reason, UnresolvedReason::NotFound);
    }

    #[test]
    fn duplicate_edges_are_deduped() {
        let f = fid("src/math.rs");
        let index = SymbolIndex::build(vec![
            sym(f, "src/math.rs", "square", "square"),
            sym(f, "src/math.rs", "mul", "mul"),
        ]);

        let (edges, _) =
            resolve_file_edges(&index, f, &[call("square", "mul"), call("square", "mul")]);

        assert_eq!(edges.len(), 1, "identical edges must be deduped");
    }
}
