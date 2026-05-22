//! Domain types, errors, and traits for the Mnemo code intelligence layer.
//!
//! This crate is dependency-free within the workspace — it defines the shared
//! vocabulary that all other crates build on.

pub mod error;
pub mod types;

// Re-export key types for convenience.
pub use error::CoreError;
pub use types::{
    Confidence, Edge, EdgeKind, FileId, Language, Range, SnapshotId, SourceRange,
    Symbol, SymbolId, SymbolKind,
};
