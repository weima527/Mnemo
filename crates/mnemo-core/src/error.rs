//! Error types for Mnemo core.
//!
//! All workspace-internal crates should use this error type
//! (or wrap it) rather than exposing raw library errors.

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
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A serialization / deserialization error.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// An operation was attempted on an unsupported state.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// A generic internal error (use sparingly).
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience result alias for the core crate.
pub type Result<T> = std::result::Result<T, CoreError>;
