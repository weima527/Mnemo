//! Domain types, errors, and traits for the Mnemo code intelligence layer.
//!
//! This crate defines the shared vocabulary that all other crates build on.

pub mod error;
pub mod ids;
pub mod types;

// Re-export key types for convenience.
pub use error::CoreError;
pub use ids::{
    FileIdentityId, ProjectId, SnapshotId, SymbolIdentityId, SymbolVersionId,
};
pub use types::{
    Confidence, Edge, EdgeKind, Language, Range, SourceRange, Symbol, SymbolKind,
};
