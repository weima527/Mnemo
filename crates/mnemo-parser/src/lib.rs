//! Tree-sitter adapters and symbol extraction.
//!
//! This crate wraps tree-sitter language grammars and provides
//! a uniform interface for extracting symbols and edges from source files.
//!
//! Initial MVP target: Rust (via `tree-sitter-rust`).

use mnemo_core::{Confidence, FileIdentityId, Language, Range, Symbol, SymbolIdentityId, SymbolKind};
use std::path::Path;

/// A parsed source file with extracted symbols and their source ranges.
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// The file that was parsed.
    pub file_id: FileIdentityId,
    /// Detected language.
    pub language: Language,
    /// Symbols found in the file.
    pub symbols: Vec<Symbol>,
    /// Any parse errors encountered (non-fatal).
    pub errors: Vec<ParseError>,
}

/// A non-fatal parse error for a specific location.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub line: u32,
    pub column: u32,
}

/// Parse a source file and return extracted symbols.
///
/// The language is auto-detected from the file extension.
/// Unknown or unsupported languages return an empty `ParseResult`
/// with a single `ParseError`.
pub fn parse_file(file_id: FileIdentityId, path: &Path, source: &str) -> ParseResult {
    let language = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(Language::from_extension);

    match language {
        Some(Language::Rust) => parse_rust(file_id, source),
        Some(Language::TypeScript) => parse_typescript(file_id, source),
        None => ParseResult {
            file_id,
            language: Language::Rust, // placeholder
            symbols: Vec::new(),
            errors: vec![ParseError {
                message: format!(
                    "unsupported file extension: {:?}",
                    path.extension()
                ),
                line: 1,
                column: 1,
            }],
        },
    }
}

/// Stub: parse Rust source code via tree-sitter.
///
/// TODO: Integrate `tree-sitter-rust` grammar and walk the CST.
fn parse_rust(file_id: FileIdentityId, _source: &str) -> ParseResult {
    // Placeholder — returns an empty result.
    // Real implementation will:
    // 1. Set the tree-sitter parser language to Rust.
    // 2. Parse source → concrete syntax tree.
    // 3. Walk the tree, extracting `Symbol` and `Range` for each definition.
    tracing::debug!(%file_id, "parse_rust: not yet implemented");
    ParseResult {
        file_id,
        language: Language::Rust,
        symbols: Vec::new(),
        errors: Vec::new(),
    }
}

/// Stub: parse TypeScript source code via tree-sitter.
fn parse_typescript(file_id: FileIdentityId, _source: &str) -> ParseResult {
    // Placeholder — same structure as parse_rust.
    tracing::debug!(%file_id, "parse_typescript: not yet implemented");
    ParseResult {
        file_id,
        language: Language::TypeScript,
        symbols: Vec::new(),
        errors: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_detection_rust() {
        assert_eq!(
            Language::from_extension("rs"),
            Some(Language::Rust)
        );
    }

    #[test]
    fn language_detection_typescript() {
        assert_eq!(
            Language::from_extension("ts"),
            Some(Language::TypeScript)
        );
    }

    #[test]
    fn language_detection_unknown() {
        assert_eq!(Language::from_extension("py"), None);
    }

    #[test]
    fn parse_unsupported_extension() {
        let result = parse_file(
            FileIdentityId::ZERO,
            Path::new("main.py"),
            "print('hello')",
        );
        assert!(result.symbols.is_empty());
        assert_eq!(result.errors.len(), 1);
    }
}
