//! Shared domain types for the Mnemo code intelligence layer.
//!
//! These types form the vocabulary used across all crates:
//! symbols, files, edges, snapshots, overlays, etc.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// Unique identifier for a file within a repository index.
pub type FileId = Uuid;

/// Unique identifier for a symbol (function, struct, module, etc.).
pub type SymbolId = Uuid;

/// Unique identifier for a snapshot (immutable point-in-time graph view).
pub type SnapshotId = Uuid;

// ---------------------------------------------------------------------------
// Language
// ---------------------------------------------------------------------------

/// Supported programming languages.
///
/// The initial MVP targets a single language; additional variants
/// are added as tree-sitter grammars are integrated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Rust source files (`.rs`).
    Rust,
    /// TypeScript / JavaScript source files (`.ts`, `.tsx`, `.js`, `.jsx`).
    TypeScript,
}

impl Language {
    /// Guess the language from a file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Self::Rust),
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(Self::TypeScript),
            _ => None,
        }
    }

    /// File extensions commonly associated with this language.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["rs"],
            Self::TypeScript => &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
        }
    }
}

// ---------------------------------------------------------------------------
// Source range
// ---------------------------------------------------------------------------

/// A line-column range in a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    /// 0-based start byte offset.
    pub start_byte: usize,
    /// 0-based end byte offset (exclusive).
    pub end_byte: usize,
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based end line (inclusive).
    pub end_line: u32,
    /// 1-based start column.
    pub start_column: u32,
    /// 1-based end column.
    pub end_column: u32,
}

/// A source range together with the file it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRange {
    /// The file this range is in.
    pub file_id: FileId,
    /// The byte and line-column range.
    pub range: Range,
}

// ---------------------------------------------------------------------------
// Symbol
// ---------------------------------------------------------------------------

/// Kinds of symbols the system can extract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    /// A function or method definition.
    Function,
    /// A struct / record definition.
    Struct,
    /// An enum definition.
    Enum,
    /// A type alias.
    TypeAlias,
    /// A trait / interface definition.
    Trait,
    /// An implementation block.
    Impl,
    /// A module / namespace declaration.
    Module,
    /// A variable or constant.
    Variable,
    /// A macro definition.
    Macro,
    /// Any other symbol kind not yet classified.
    Other,
}

/// A named symbol extracted from source code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    /// Unique identifier.
    pub id: SymbolId,
    /// Human-readable name (e.g. `parse_file`).
    pub name: String,
    /// Fully-qualified name when available (e.g. `mnemo_parser::parse_file`).
    pub qualified_name: Option<String>,
    /// What kind of symbol this is.
    pub kind: SymbolKind,
    /// The file where this symbol is defined.
    pub file_id: FileId,
    /// The source location of the definition.
    pub definition_range: Range,
    /// Language of the source file.
    pub language: Language,
}

// ---------------------------------------------------------------------------
// Edges
// ---------------------------------------------------------------------------

/// Kinds of directed edges between symbols (or files).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Symbol A calls symbol B.
    Calls,
    /// Symbol A imports / uses symbol B.
    Imports,
    /// Symbol A contains symbol B (module → child).
    Contains,
    /// Symbol A is defined / implemented in terms of symbol B (e.g. implements).
    Implements,
    /// File A depends on file B (import-level).
    FileDepends,
}

/// A directed edge in the code graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// Source node (symbol or file id).
    pub from: SymbolId,
    /// Target node (symbol or file id).
    pub to: SymbolId,
    /// Relationship kind.
    pub kind: EdgeKind,
    /// Optional source range where the edge is evidenced.
    pub location: Option<SourceRange>,
}

// ---------------------------------------------------------------------------
// Confidence
// ---------------------------------------------------------------------------

/// Confidence level for an extracted fact or inference.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Confidence(f64);

impl Confidence {
    /// A fact backed by a direct source definition.
    pub const CERTAIN: Self = Self(1.0);
    /// A fact inferred heuristically with high confidence.
    pub const HIGH: Self = Self(0.85);
    /// A fact inferred with moderate confidence.
    pub const MEDIUM: Self = Self(0.6);
    /// A weak signal, treat as a hint only.
    pub const LOW: Self = Self(0.3);
    /// No confidence — the fact should not be used.
    pub const NONE: Self = Self(0.0);

    /// Create a new confidence value, clamped to [0.0, 1.0].
    pub fn new(value: f64) -> Self {
        Self(value.clamp(0.0, 1.0))
    }

    /// Return the raw f64 value.
    pub fn value(self) -> f64 {
        self.0
    }
}
