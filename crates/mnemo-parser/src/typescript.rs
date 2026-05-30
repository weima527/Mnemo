//! TypeScript / JavaScript symbol/edge extraction via tree-sitter.
//!
//! Parallel to the Rust extractor: walks the syntax tree produced by
//! `tree-sitter-typescript` (TypeScript grammar for `.ts`/`.js`/`.mjs`/`.cjs`,
//! TSX grammar for `.tsx`/`.jsx`) and emits [`RawSymbol`]s and [`RawEdge`]s.
//!
//! Qualified-name policy: `class`, `interface`, and `namespace` scopes
//! contribute a `::`-joined prefix (cross-language consistency with the Rust
//! extractor — TS's native `.` would diverge by language). So a method
//! `magnitude` inside `class Vec2` becomes `Vec2::magnitude`.

use crate::{ParseError, ParseResult};
use mnemo_core::{EdgeKind, FileIdentityId, Language, Range, RawEdge, RawSymbol, SymbolKind};
use tree_sitter::{Node, Parser};

/// Parse TypeScript/JavaScript `source` and extract symbols + intra-file edges.
/// `extension` picks the grammar: TSX (which is a superset of TS, accepts JSX)
/// for `.tsx`/`.jsx`; TypeScript (also handles plain JS) for the rest.
pub(crate) fn extract(file_id: FileIdentityId, source: &str, extension: &str) -> ParseResult {
    let mut parser = Parser::new();
    let grammar = match extension {
        "tsx" | "jsx" => tree_sitter::Language::new(tree_sitter_typescript::LANGUAGE_TSX),
        _ => tree_sitter::Language::new(tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
    };
    if let Err(e) = parser.set_language(&grammar) {
        return error_result(file_id, format!("failed to load TypeScript grammar: {e}"));
    }

    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return error_result(file_id, "tree-sitter returned no parse tree".to_string()),
    };

    let mut visitor = TsVisitor {
        source: source.as_bytes(),
        symbols: Vec::new(),
        edges: Vec::new(),
    };
    let mut scope: Vec<String> = Vec::new();
    visitor.walk(tree.root_node(), &mut scope, None);

    ParseResult {
        file_id,
        language: Language::TypeScript,
        symbols: visitor.symbols,
        edges: visitor.edges,
        errors: Vec::new(),
    }
}

fn error_result(file_id: FileIdentityId, message: String) -> ParseResult {
    ParseResult {
        file_id,
        language: Language::TypeScript,
        symbols: Vec::new(),
        edges: Vec::new(),
        errors: vec![ParseError {
            message,
            line: 1,
            column: 1,
        }],
    }
}

struct TsVisitor<'a> {
    source: &'a [u8],
    symbols: Vec<RawSymbol>,
    edges: Vec<RawEdge>,
}

impl TsVisitor<'_> {
    /// Recursively walk children of `node`.
    ///
    /// `scope` is the current `::`-joined definition path (class / interface /
    /// namespace). `enclosing` is the qualified name of the nearest enclosing
    /// function-like definition; intra-file references found below are
    /// attributed to it.
    fn walk(&mut self, node: Node, scope: &mut Vec<String>, enclosing: Option<&str>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "function_declaration" | "generator_function_declaration" => {
                    if let Some(name) = self.field_text(child, "name") {
                        let qn = qualify(scope, &name);
                        self.push_symbol(child, &name, &qn, SymbolKind::Function);
                        self.walk(child, scope, Some(&qn));
                    } else {
                        self.walk(child, scope, enclosing);
                    }
                }
                "class_declaration" | "abstract_class_declaration" => {
                    if let Some(name) = self.field_text(child, "name") {
                        let qn = qualify(scope, &name);
                        self.push_symbol(child, &name, &qn, SymbolKind::Struct);
                        self.emit_implements(child, &qn);
                        scope.push(name);
                        self.walk(child, scope, enclosing);
                        scope.pop();
                    } else {
                        self.walk(child, scope, enclosing);
                    }
                }
                "interface_declaration" => {
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
                "type_alias_declaration" => {
                    self.named_symbol(child, scope, SymbolKind::TypeAlias);
                    self.walk(child, scope, enclosing);
                }
                "enum_declaration" => {
                    self.named_symbol(child, scope, SymbolKind::Enum);
                    self.walk(child, scope, enclosing);
                }
                "internal_module" | "module" => {
                    // TS `namespace foo {}` / `module 'foo' {}`
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
                "method_definition" | "method_signature" => {
                    if let Some(name) = self.field_text(child, "name") {
                        let qn = qualify(scope, &name);
                        self.push_symbol(child, &name, &qn, SymbolKind::Function);
                        self.walk(child, scope, Some(&qn));
                    } else {
                        self.walk(child, scope, enclosing);
                    }
                }
                "lexical_declaration" | "variable_declaration" => {
                    self.handle_var_decl(child, scope);
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

    /// `const x = ...` / `let x = ...` / `var x = ...`.
    /// When the initializer is a function-like expression, the declarator is
    /// recorded as a `Function` and its body walked with the new enclosing;
    /// otherwise it's a `Variable`.
    fn handle_var_decl(&mut self, node: Node, scope: &mut Vec<String>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() != "variable_declarator" {
                continue;
            }
            let Some(name) = self.field_text(child, "name") else {
                continue;
            };
            let value = child.child_by_field_name("value");
            let is_function_like = matches!(
                value.map(|v| v.kind()),
                Some("arrow_function" | "function_expression" | "function")
            );
            let kind = if is_function_like {
                SymbolKind::Function
            } else {
                SymbolKind::Variable
            };
            let qn = qualify(scope, &name);
            self.push_symbol(child, &name, &qn, kind);
            if is_function_like {
                if let Some(body) = value {
                    self.walk(body, scope, Some(&qn));
                }
            }
        }
    }

    /// Emit `Implements` edges for `class C implements I1, I2`.
    fn emit_implements(&mut self, class_node: Node, class_qn: &str) {
        let mut cursor = class_node.walk();
        for child in class_node.children(&mut cursor) {
            if child.kind() != "class_heritage" {
                continue;
            }
            let mut hc = child.walk();
            for h in child.children(&mut hc) {
                if h.kind() != "implements_clause" {
                    continue;
                }
                let mut ic = h.walk();
                for t in h.children(&mut ic) {
                    let name = match t.kind() {
                        "type_identifier" | "nested_type_identifier" => self.node_text(t),
                        "generic_type" => self.field_text(t, "name"),
                        _ => None,
                    };
                    if let Some(n) = name {
                        self.edges.push(RawEdge {
                            from_qualified_name: class_qn.to_string(),
                            to_name: last_segment(&n),
                            kind: EdgeKind::Implements,
                            location: Some(node_range(class_node)),
                        });
                    }
                }
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

    fn callee_name(&self, call: Node) -> Option<String> {
        let func = call.child_by_field_name("function")?;
        self.leaf_callee_name(func)
    }

    /// Reduce a callee expression to the bare name being invoked.
    fn leaf_callee_name(&self, node: Node) -> Option<String> {
        match node.kind() {
            // `foo()`
            "identifier" | "property_identifier" => self.node_text(node),
            // `obj.method()` → `method`
            "member_expression" => self.field_text(node, "property"),
            // `obj?.method()` → `method`
            "subscript_expression" => self.field_text(node, "index"),
            // Parenthesised callee, e.g. `(getFn())()` — skip to inner expression.
            "parenthesized_expression" => {
                node.named_child(0).and_then(|n| self.leaf_callee_name(n))
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

/// The last `::`-or-`.`-separated segment of a path (e.g. `a.b.C` → `C`).
fn last_segment(path: &str) -> String {
    let trimmed = path.trim();
    trimmed
        .rsplit(['.', ':'])
        .next()
        .unwrap_or(trimmed)
        .trim()
        .to_string()
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

    fn parse(src: &str, ext: &str) -> ParseResult {
        extract(FileIdentityId::ZERO, src, ext)
    }

    fn names(result: &ParseResult, kind: SymbolKind) -> Vec<String> {
        result
            .symbols
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| s.qualified_name.clone())
            .collect()
    }

    fn has_edge(result: &ParseResult, kind: EdgeKind, from_contains: &str, to: &str) -> bool {
        result.edges.iter().any(|e| {
            e.kind == kind && e.from_qualified_name.contains(from_contains) && e.to_name == to
        })
    }

    #[test]
    fn extracts_free_functions_and_const_arrow() {
        let src = "\
export function add(a: number, b: number): number { return a + b; }
export const square = (n: number): number => n * n;
const dbl = function(n: number) { return n * 2; };
";
        let result = parse(src, "ts");
        let fns = names(&result, SymbolKind::Function);
        assert!(fns.contains(&"add".to_string()), "fns: {fns:?}");
        assert!(fns.contains(&"square".to_string()), "fns: {fns:?}");
        assert!(fns.contains(&"dbl".to_string()), "fns: {fns:?}");
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    }

    #[test]
    fn same_file_call_edge_is_extracted() {
        let src = "\
function mul(a: number, b: number) { return a * b; }
function square(n: number) { return mul(n, n); }
";
        let result = parse(src, "ts");
        assert!(
            has_edge(&result, EdgeKind::Calls, "square", "mul"),
            "expected square -> mul Calls, got {:?}",
            result.edges
        );
    }

    #[test]
    fn class_method_is_qualified_and_calls_are_attributed() {
        let src = "\
interface Length { length(): number }
class Vec2 implements Length {
    constructor(public x: number, public y: number) {}
    magnitude(): number { return Math.sqrt(this.x * this.x + this.y * this.y); }
    length(): number { return this.magnitude(); }
}
";
        let result = parse(src, "ts");

        assert!(
            names(&result, SymbolKind::Trait).contains(&"Length".to_string()),
            "expected `Length` interface symbol, got {:?}",
            names(&result, SymbolKind::Trait)
        );
        assert!(
            names(&result, SymbolKind::Struct).contains(&"Vec2".to_string()),
            "expected `Vec2` class symbol, got {:?}",
            names(&result, SymbolKind::Struct)
        );
        // Methods are qualified under the class.
        let fns = names(&result, SymbolKind::Function);
        assert!(
            fns.contains(&"Vec2::magnitude".to_string()),
            "expected `Vec2::magnitude`, got {fns:?}"
        );
        assert!(
            fns.contains(&"Vec2::length".to_string()),
            "expected `Vec2::length`, got {fns:?}"
        );
        // `length` calls `this.magnitude()` → Calls edge attributed to it.
        assert!(
            has_edge(&result, EdgeKind::Calls, "Vec2::length", "magnitude"),
            "expected Vec2::length -> magnitude Calls, got {:?}",
            result.edges
        );
        // `class Vec2 implements Length` → Implements edge.
        assert!(
            has_edge(&result, EdgeKind::Implements, "Vec2", "Length"),
            "expected Vec2 implements Length, got {:?}",
            result.edges
        );
    }

    #[test]
    fn tsx_grammar_handles_jsx() {
        let src = "\
export function Greet({ name }: { name: string }) {
    return <div>Hello, {name}</div>;
}
";
        let result = parse(src, "tsx");
        assert!(
            names(&result, SymbolKind::Function).contains(&"Greet".to_string()),
            "expected `Greet` function symbol, got {:?}",
            names(&result, SymbolKind::Function)
        );
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    }

    #[test]
    fn namespace_contributes_to_qualified_name() {
        let src = "\
namespace geom {
    export function distance(a: number, b: number): number { return Math.abs(a - b); }
}
";
        let result = parse(src, "ts");
        assert!(
            names(&result, SymbolKind::Module).contains(&"geom".to_string()),
            "expected `geom` namespace symbol, got modules: {:?}",
            names(&result, SymbolKind::Module)
        );
        assert!(
            names(&result, SymbolKind::Function).contains(&"geom::distance".to_string()),
            "expected `geom::distance`, got {:?}",
            names(&result, SymbolKind::Function)
        );
    }

    #[test]
    fn malformed_source_does_not_panic() {
        let result = parse("function broken( { let x = ; ", "ts");
        let _ = result.symbols.len();
    }
}
