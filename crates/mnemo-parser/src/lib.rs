//! Tree-sitter adapters and symbol extraction.
//!
//! This crate wraps tree-sitter language grammars and provides a uniform
//! interface for extracting symbols and edges from source files. The output is
//! *raw* (see [`mnemo_core::RawSymbol`] / [`mnemo_core::RawEdge`]): the parser
//! works one file at a time and has no project context, so identity/version
//! IDs and cross-file resolution are left to the index pipeline.
//!
//! Supported languages: Rust (via `tree-sitter-rust`) and TypeScript /
//! JavaScript (via `tree-sitter-typescript`, with TSX grammar for `.tsx` /
//! `.jsx` files).

mod rust;
mod typescript;

use mnemo_core::{FileIdentityId, Language, RawEdge, RawSymbol};
use std::path::Path;

/// A parsed source file with extracted symbols and references.
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// The file that was parsed.
    pub file_id: FileIdentityId,
    /// Detected language.
    pub language: Language,
    /// Symbol definitions found in the file (pre-identity).
    pub symbols: Vec<RawSymbol>,
    /// Intra-file references found in the file (unresolved targets).
    pub edges: Vec<RawEdge>,
    /// Any parse errors encountered (non-fatal).
    pub errors: Vec<ParseError>,
}

/// A non-fatal parse error for a specific location.
#[derive(Debug, Clone)]
pub struct ParseError {
    /// Human-readable description of the problem.
    pub message: String,
    /// 1-based line where the problem was detected.
    pub line: u32,
    /// 1-based column where the problem was detected.
    pub column: u32,
}

/// Parse a source file and return extracted symbols and edges.
///
/// The language is auto-detected from the file extension. Unknown or
/// unsupported languages return an empty `ParseResult` with a single
/// `ParseError`.
pub fn parse_file(file_id: FileIdentityId, path: &Path, source: &str) -> ParseResult {
    let language = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(Language::from_extension);

    match language {
        Some(Language::Rust) => rust::extract(file_id, source),
        Some(Language::TypeScript) => {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("ts");
            typescript::extract(file_id, source, ext)
        }
        None => ParseResult {
            file_id,
            language: Language::Rust, // placeholder
            symbols: Vec::new(),
            edges: Vec::new(),
            errors: vec![ParseError {
                message: format!("unsupported file extension: {:?}", path.extension()),
                line: 1,
                column: 1,
            }],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_detection_rust() {
        assert_eq!(Language::from_extension("rs"), Some(Language::Rust));
    }

    #[test]
    fn language_detection_typescript() {
        assert_eq!(Language::from_extension("ts"), Some(Language::TypeScript));
    }

    #[test]
    fn language_detection_unknown() {
        assert_eq!(Language::from_extension("py"), None);
    }

    #[test]
    fn parse_unsupported_extension() {
        let result = parse_file(FileIdentityId::ZERO, Path::new("main.py"), "print('hello')");
        assert!(result.symbols.is_empty());
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn parse_rust_file_extracts_symbols() {
        let result = parse_file(
            FileIdentityId::ZERO,
            Path::new("lib.rs"),
            "pub fn answer() -> i64 { 42 }\n",
        );
        assert_eq!(result.language, Language::Rust);
        assert_eq!(result.symbols.len(), 1);
        assert_eq!(result.symbols[0].name, "answer");
    }

    #[test]
    fn parse_typescript_file_extracts_symbols() {
        let result = parse_file(
            FileIdentityId::ZERO,
            Path::new("util.ts"),
            "export function answer(): number { return 42; }\n",
        );
        assert_eq!(result.language, Language::TypeScript);
        assert!(
            result.symbols.iter().any(|s| s.name == "answer"),
            "expected `answer`, got {:?}",
            result.symbols
        );
        assert!(result.errors.is_empty());
    }

    #[test]
    fn parse_tsx_file_uses_tsx_grammar() {
        let result = parse_file(
            FileIdentityId::ZERO,
            Path::new("Btn.tsx"),
            "export function Btn() { return <button>ok</button>; }\n",
        );
        assert_eq!(result.language, Language::TypeScript);
        assert!(result.symbols.iter().any(|s| s.name == "Btn"));
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    }
}
