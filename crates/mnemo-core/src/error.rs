//! Error types for Mnemo core.
//!
//! All workspace-internal crates should use this error type
//! (or wrap it) rather than exposing raw library errors.
//!
//! Variant guidelines (per DESIGN §9 and AGENTS.md 10.2):
//! - Never wrap downstream errors with `Internal(format!(...))`. Add a variant.
//! - Artificial `io::Error` wrapping is banned — use real `#[from]` impls.
//! - New variants are added at the end (integer discriminants must never shift).

use std::path::PathBuf;
use thiserror::Error;

/// Unified error type for the Mnemo core layer.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A required resource was not found.
    #[error("{0} not found: {1}")]
    NotFound(&'static str, String),

    /// Input validation failed.
    #[error("invalid {field}: {reason}")]
    InvalidInput {
        /// The field that failed validation.
        field: &'static str,
        /// Why it failed.
        reason: String,
    },

    /// An I/O operation failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// A SQLite operation failed.
    #[cfg(feature = "sqlite")]
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// A git operation failed.
    #[cfg(feature = "git")]
    #[error("git: {0}")]
    Git(#[from] git2::Error),

    /// A parse error for a specific file.
    #[error("parse {file:?}: {message}")]
    Parse {
        /// File that failed to parse.
        file: PathBuf,
        /// Human-readable message.
        message: String,
    },

    /// Token or memory budget exceeded.
    #[error("budget exceeded: needed {needed}, available {available}")]
    BudgetExceeded {
        /// Amount required.
        needed: u64,
        /// Amount available.
        available: u64,
    },

    /// A project tenant is unavailable (not loaded, evicted, or corrupted).
    #[error("tenant {project:?} unavailable: {reason}")]
    TenantUnavailable {
        /// Project identifier (string form until M0.2 replaces with ProjectId newtype).
        project: String,
        /// Why it's unavailable.
        reason: String,
    },

    /// Schema migration failed.
    #[error("schema migration {from} → {to}: {reason}")]
    Migration {
        /// Source version.
        from: u32,
        /// Target version.
        to: u32,
        /// Why it failed.
        reason: String,
    },

    /// A serialization / deserialization error.
    #[error("serialization: {0}")]
    Serialization(String),

    /// An operation was attempted on an unsupported state.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// A generic internal error (use sparingly — only for truly unreachable states).
    #[error("internal: {0}")]
    Internal(String),
}

/// Convenience result alias for the core crate.
pub type Result<T> = std::result::Result<T, CoreError>;

// ---------------------------------------------------------------------------
// Cross-version compatibility constructors
// ---------------------------------------------------------------------------

impl CoreError {
    /// Construct a `Parse` error.
    pub fn parse_error(file: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::Parse {
            file: file.into(),
            message: message.into(),
        }
    }

    /// Construct a `BudgetExceeded` error.
    pub fn budget_exceeded(needed: u64, available: u64) -> Self {
        Self::BudgetExceeded { needed, available }
    }

    /// Construct a `TenantUnavailable` error.
    pub fn tenant_unavailable(project: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::TenantUnavailable {
            project: project.into(),
            reason: reason.into(),
        }
    }

    /// Construct a `Migration` error.
    pub fn migration_error(from: u32, to: u32, reason: impl Into<String>) -> Self {
        Self::Migration {
            from,
            to,
            reason: reason.into(),
        }
    }
}
