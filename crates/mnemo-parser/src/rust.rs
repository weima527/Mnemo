//! Rust symbol/edge extraction via tree-sitter.
//!
//! Walks the concrete syntax tree produced by `tree-sitter-rust` and emits
//! [`RawSymbol`]s (definitions) and [`RawEdge`]s (intra-file references). The
//! output is *raw*: targets are unresolved names and qualified names are
//! file-local (no crate/module prefix), because the parser sees one file at a
//! time and has no project context. The resolver (M1.2) and index pipeline
//! (M1.4) finalize these into concrete `Symbol`/`Edge` rows.
//!
//! Qualified-name policy (DESIGN-aligned MVP): `mod`, `impl`, and `trait`
//! scopes contribute a `::`-joined prefix; plain functions do not nest other
//! symbols. So a method `manhattan` inside `impl Point` becomes
//! `Point::manhattan`.

use crate::{ParseError, ParseResult};
use mnemo_core::{EdgeKind, FileIdentityId, Language, Range, RawEdge, RawSymbol, SymbolKind};
use tree_sitter::{Node, Parser};

/// Parse Rust `source` and extract symbols + intra-file edges.
pub(crate) fn extract(file_id: FileIdentityId, source: &str) -> ParseResult {
    let mut parser = Parser::new();
    let ts_language = tree_sitter::Language::new(tree_sitter_rust::LANGUAGE);
    if let Err(e) = parser.set_language(&ts_language) {
        return error_result(file_id, format!("failed to load Rust grammar: {e}"));
    }

    let tree = match parser.parse(source, None) {
        Some(tree) => tree,
        None => return error_result(file_id, "tree-sitter returned no parse tree".to_string()),
    };

    let mut visitor = RustVisitor {
        source: source.as_bytes(),
        symbols: Vec::new(),
        edges: Vec::new(),
    };
    let mut scope: Vec<String> = Vec::new();
    visitor.walk(tree.root_node(), &mut scope, None);

    ParseResult {
        file_id,
        language: Language::Rust,
        symbols: visitor.symbols,
        edges: visitor.edges,
        errors: Vec::new(),
    }
}

/// Build a `ParseResult` carrying a single fatal parse error.
fn error_result(file_id: FileIdentityId, message: String) -> ParseResult {
    ParseResult {
        file_id,
        language: Language::Rust,
        symbols: Vec::new(),
        edges: Vec::new(),
        errors: vec![ParseError {
            message,
            line: 1,
            column: 1,
        }],
    }
}

struct RustVisitor<'a> {
    source: &'a [u8],
    symbols: Vec<RawSymbol>,
    edges: Vec<RawEdge>,
}

impl RustVisitor<'_> {
    /// Recursively walk `node`'s children.
    ///
    /// `scope` is the current `::`-joined definition path (mod/impl/trait).
    /// `enclosing` is the qualified name of the nearest enclosing function, to
    /// which any reference edges (e.g. calls) found here are attributed.
    fn walk(&mut self, node: Node, scope: &mut Vec<String>, enclosing: Option<&str>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "function_item" | "function_signature_item" => {
                    if let Some(name) = self.field_text(child, "name") {
                        let qn = qualify(scope, &name);
                        self.push_symbol(child, &name, &qn, SymbolKind::Function);
                        // References inside the body belong to this function.
                        self.walk(child, scope, Some(&qn));
                    } else {
                        self.walk(child, scope, enclosing);
                    }
                }
                "struct_item" | "union_item" => {
                    self.named_symbol(child, scope, SymbolKind::Struct);
                    self.walk(child, scope, enclosing);
                }
                "enum_item" => {
                    self.named_symbol(child, scope, SymbolKind::Enum);
                    self.walk(child, scope, enclosing);
                }
                "trait_item" => {
                    if let Some(name) = self.field_text(child, "name") {
                        let qn = qualify(scope, &name);
                        self.push_symbol(child, &name, &qn, SymbolKind::Trait);
                        scope.push(name);
                        self.walk(child, scope, enclosing);
                        scope.pop();
                    } else {
                        self.walk(child, scope, enclosing);
                    }
                }
                "impl_item" => {
                    let type_name = self
                        .field_text(child, "type")
                        .map(|t| last_segment(&t))
                        .unwrap_or_else(|| "impl".to_string());
                    let qn = qualify(scope, &type_name);
                    self.push_symbol(child, &type_name, &qn, SymbolKind::Impl);
                    // `impl Trait for Type` → Implements edge.
                    if let Some(trait_name) = self.field_text(child, "trait") {
                        self.edges.push(RawEdge {
                            from_qualified_name: qn.clone(),
                            to_name: last_segment(&trait_name),
                            kind: EdgeKind::Implements,
                            location: Some(node_range(child)),
                        });
                    }
                    scope.push(type_name);
                    self.walk(child, scope, enclosing);
                    scope.pop();
                }
                "mod_item" => {
                    if let Some(name) = self.field_text(child, "name") {
                        let qn = qualify(scope, &name);
                        self.push_symbol(child, &name, &qn, SymbolKind::Module);
                        scope.push(name);
                        self.walk(child, scope, enclosing);
                        scope.pop();
                    } else {
                        self.walk(child, scope, enclosing);
                    }
                }
                "const_item" | "static_item" => {
                    self.named_symbol(child, scope, SymbolKind::Variable);
                    self.walk(child, scope, enclosing);
                }
                "type_item" => {
                    self.named_symbol(child, scope, SymbolKind::TypeAlias);
                    self.walk(child, scope, enclosing);
                }
                "macro_definition" => {
                    self.named_symbol(child, scope, SymbolKind::Macro);
                    self.walk(child, scope, enclosing);
                }
                "call_expression" => {
                    if let (Some(from), Some(to)) = (enclosing, self.callee_name(child)) {
                        self.edges.push(RawEdge {
                            from_qualified_name: from.to_string(),
                            to_name: to,
                            kind: EdgeKind::Calls,
                            location: Some(node_range(child)),
                        });
                    }
                    self.walk(child, scope, enclosing);
                }
                _ => self.walk(child, scope, enclosing),
            }
        }
    }

    /// Emit a symbol whose name is in the `name` field of `node`.
    fn named_symbol(&mut self, node: Node, scope: &[String], kind: SymbolKind) {
        if let Some(name) = self.field_text(node, "name") {
            let qn = qualify(scope, &name);
            self.push_symbol(node, &name, &qn, kind);
        }
    }

    fn push_symbol(&mut self, node: Node, name: &str, qualified_name: &str, kind: SymbolKind) {
        let content = node.utf8_text(self.source).unwrap_or("");
        self.symbols.push(RawSymbol {
            name: name.to_string(),
            qualified_name: qualified_name.to_string(),
            kind,
            definition_range: node_range(node),
            content_hash: blake3::hash(content.as_bytes()),
        });
    }

    /// Extract the callee name from a `call_expression` node.
    fn callee_name(&self, call: Node) -> Option<String> {
        let func = call.child_by_field_name("function")?;
        self.leaf_callee_name(func)
    }

    /// Reduce a callee expression to the bare name being invoked.
    fn leaf_callee_name(&self, node: Node) -> Option<String> {
        match node.kind() {
            // `foo()`
            "identifier" => self.node_text(node),
            // `x.method()` → `method`
            "field_expression" => self.field_text(node, "field"),
            // `module::foo()` → `foo`
            "scoped_identifier" => self.field_text(node, "name"),
            // `foo::<T>()` → recurse into the inner function expression
            "generic_function" => {
                let inner = node.child_by_field_name("function")?;
                self.leaf_callee_name(inner)
            }
            _ => None,
        }
    }

    fn field_text(&self, node: Node, field: &str) -> Option<String> {
        node.child_by_field_name(field)
            .and_then(|n| self.node_text(n))
    }

    fn node_text(&self, node: Node) -> Option<String> {
        node.utf8_text(self.source).ok().map(|s| s.to_string())
    }
}

/// Join a scope path and a name into a `::`-qualified name.
fn qualify(scope: &[String], name: &str) -> String {
    if scope.is_empty() {
        name.to_string()
    } else {
        format!("{}::{}", scope.join("::"), name)
    }
}

/// The last `::`-separated segment of a path (e.g. `a::b::C` → `C`).
fn last_segment(path: &str) -> String {
    path.rsplit("::").next().unwrap_or(path).trim().to_string()
}

/// Convert a tree-sitter node span to a [`Range`] (1-based lines/columns).
fn node_range(node: Node) -> Range {
    let start = node.start_position();
    let end = node.end_position();
    Range {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: start.row as u32 + 1,
        end_line: end.row as u32 + 1,
        start_column: start.column as u32 + 1,
        end_column: end.column as u32 + 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> ParseResult {
        extract(FileIdentityId::ZERO, src)
    }

    fn names(result: &ParseResult, kind: SymbolKind) -> Vec<String> {
        result
            .symbols
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| s.qualified_name.clone())
            .collect()
    }

    fn has_call(result: &ParseResult, from_contains: &str, to: &str) -> bool {
        result.edges.iter().any(|e| {
            e.kind == EdgeKind::Calls
                && e.from_qualified_name.contains(from_contains)
                && e.to_name == to
        })
    }

    #[test]
    fn extracts_free_functions() {
        // Mirrors tests/fixtures/basic_rust/src/math.rs.
        let src = "\
pub fn add(a: i64, b: i64) -> i64 { a + b }
pub fn mul(a: i64, b: i64) -> i64 { a * b }
pub fn square(n: i64) -> i64 { mul(n, n) }
";
        let result = parse(src);
        let fns = names(&result, SymbolKind::Function);
        assert!(fns.contains(&"add".to_string()), "fns: {fns:?}");
        assert!(fns.contains(&"mul".to_string()), "fns: {fns:?}");
        assert!(fns.contains(&"square".to_string()), "fns: {fns:?}");
        assert!(result.errors.is_empty());
    }

    #[test]
    fn same_file_call_edge_is_extracted() {
        let src = "fn mul(a: i64, b: i64) -> i64 { a * b }\nfn square(n: i64) -> i64 { mul(n, n) }\n";
        let result = parse(src);
        assert!(
            has_call(&result, "square", "mul"),
            "expected square -> mul Calls edge, got {:?}",
            result.edges
        );
    }

    #[test]
    fn impl_method_is_qualified_and_calls_are_attributed() {
        // Mirrors tests/fixtures/basic_rust/src/lib.rs.
        let src = "\
pub mod math;
pub struct Point { pub x: i64, pub y: i64 }
impl Point {
    pub fn manhattan(&self) -> i64 { math::add(self.x.abs(), self.y.abs()) }
}
";
        let result = parse(src);

        assert!(
            names(&result, SymbolKind::Module).contains(&"math".to_string()),
            "expected `math` module symbol"
        );
        assert!(
            names(&result, SymbolKind::Struct).contains(&"Point".to_string()),
            "expected `Point` struct symbol"
        );
        // Method is qualified under the impl type.
        assert!(
            names(&result, SymbolKind::Function).contains(&"Point::manhattan".to_string()),
            "expected `Point::manhattan`, got {:?}",
            names(&result, SymbolKind::Function)
        );
        // The cross-file `math::add(...)` call is attributed to the method,
        // with the rightmost segment as the (still unresolved) target.
        assert!(
            has_call(&result, "Point::manhattan", "add"),
            "expected manhattan -> add Calls edge, got {:?}",
            result.edges
        );
    }

    #[test]
    fn malformed_source_does_not_panic() {
        // tree-sitter is error-tolerant; we just must not panic.
        let result = parse("fn broken( { let x = ; ");
        let _ = result.symbols.len();
    }
}
