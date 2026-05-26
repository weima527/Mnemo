//! Shared domain types for the Mnemo code intelligence layer.
//!
//! These types form the vocabulary used across all crates:
//! symbols, files, edges, snapshots, overlays, etc.

use crate::ids::{FileIdentityId, SymbolIdentityId, SymbolVersionId};
use serde::{Deserialize, Serialize};

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
    pub file_id: FileIdentityId,
    /// The byte and line-column range.
    pub range: Range,
}

// ---------------------------------------------------------------------------
// Symbol
// ---------------------------------------------------------------------------

/// Kinds of symbols the system can extract.
///
/// **Stability contract** (per DESIGN §2.4): new variants may only be added
/// at the end. Integer discriminants must never change. Adding a variant
/// does not count as a schema migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    /// A function or method definition.
    Function = 1,
    /// A struct / record definition.
    Struct = 2,
    /// An enum definition.
    Enum = 3,
    /// A type alias.
    TypeAlias = 4,
    /// A trait / interface definition.
    Trait = 5,
    /// An implementation block.
    Impl = 6,
    /// A module / namespace declaration.
    Module = 7,
    /// A variable or constant.
    Variable = 8,
    /// A macro definition.
    Macro = 9,
    /// Any other symbol kind not yet classified.
    Other = 99,
}

impl SymbolKind {
    /// Convert to integer for SQLite storage.
    ///
    /// Returns the explicit discriminant value.
    pub fn to_db(self) -> i64 {
        self as i64
    }

    /// Convert from integer stored in SQLite.
    ///
    /// Unknown values map to `Other`.
    pub fn from_db(v: i64) -> Self {
        match v {
            1 => Self::Function,
            2 => Self::Struct,
            3 => Self::Enum,
            4 => Self::TypeAlias,
            5 => Self::Trait,
            6 => Self::Impl,
            7 => Self::Module,
            8 => Self::Variable,
            9 => Self::Macro,
            _ => Self::Other,
        }
    }
}

/// A named symbol extracted from source code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    /// Stable identity (content-derived, survives re-index).
    pub identity_id: SymbolIdentityId,
    /// Version-specific ID (changes when body content changes).
    pub version_id: SymbolVersionId,
    /// Human-readable name (e.g. `parse_file`).
    pub name: String,
    /// Fully-qualified name when available (e.g. `mnemo_parser::parse_file`).
    pub qualified_name: Option<String>,
    /// What kind of symbol this is.
    pub kind: SymbolKind,
    /// The file where this symbol is defined.
    pub file_id: FileIdentityId,
    /// The source location of the definition.
    pub definition_range: Range,
    /// Language of the source file.
    pub language: Language,
}

// ---------------------------------------------------------------------------
// Edges
// ---------------------------------------------------------------------------

/// Kinds of directed edges between symbols (or files).
///
/// **Stability contract** (per DESIGN §2.4): same as `SymbolKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Symbol A calls symbol B.
    Calls = 1,
    /// Symbol A imports / uses symbol B.
    Imports = 2,
    /// Symbol A contains symbol B (module → child).
    Contains = 3,
    /// Symbol A is defined / implemented in terms of symbol B (e.g. implements).
    Implements = 4,
    /// File A depends on file B (import-level).
    FileDepends = 5,
}

impl EdgeKind {
    /// Convert to integer for SQLite storage.
    pub fn to_db(self) -> i64 {
        self as i64
    }

    /// Convert from integer stored in SQLite.
    ///
    /// Unknown values map to `Calls` as a conservative default.
    pub fn from_db(v: i64) -> Self {
        match v {
            1 => Self::Calls,
            2 => Self::Imports,
            3 => Self::Contains,
            4 => Self::Implements,
            5 => Self::FileDepends,
            _ => Self::Calls,
        }
    }
}

/// A directed edge in the code graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// Source node (symbol identity id).
    pub from: SymbolIdentityId,
    /// Target node (symbol identity id).
    pub to: SymbolIdentityId,
    /// Relationship kind.
    pub kind: EdgeKind,
    /// Optional source range where the edge is evidenced.
    pub location: Option<SourceRange>,
}

// ---------------------------------------------------------------------------
// Raw (pre-identity) extraction output
// ---------------------------------------------------------------------------

/// A symbol as extracted by the parser, before identity/version IDs are
/// assigned.
///
/// Content-derived IDs (`SymbolIdentityId`, `SymbolVersionId`) require project
/// context — the `ProjectId` and the repo-relative file path — that the parser
/// does not have. The index pipeline finalizes a `RawSymbol` into a [`Symbol`]
/// once that context is known.
#[derive(Debug, Clone)]
pub struct RawSymbol {
    /// Human-readable name (e.g. `add`).
    pub name: String,
    /// In-file qualified name (mod / impl / trait nesting joined by `::`, e.g.
    /// `Point::manhattan`). The crate- and file-level module prefix is added
    /// downstream by the resolver, which knows the file's module path.
    pub qualified_name: String,
    /// What kind of symbol this is.
    pub kind: SymbolKind,
    /// Source location of the definition.
    pub definition_range: Range,
    /// blake3 hash of the symbol's source text; feeds the `SymbolVersionId`.
    pub content_hash: blake3::Hash,
}

/// A reference between symbols as extracted by the parser, before the target
/// is resolved to a concrete [`SymbolIdentityId`].
///
/// The origin is known (the enclosing definition's qualified name), but the
/// target is only a raw name. The resolver (M1.2) turns this into an [`Edge`].
#[derive(Debug, Clone)]
pub struct RawEdge {
    /// In-file qualified name of the symbol the reference originates from.
    pub from_qualified_name: String,
    /// The raw (unresolved) name being referenced (e.g. a callee `mul`).
    pub to_name: String,
    /// Relationship kind.
    pub kind: EdgeKind,
    /// Source range where the reference appears, if known.
    pub location: Option<Range>,
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
