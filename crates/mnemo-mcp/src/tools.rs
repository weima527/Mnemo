//! MCP tool argument schemas.
//!
//! Each struct is the parameter type for one tool; `schemars::JsonSchema`
//! generates the MCP tool input schema and `serde::Deserialize` parses the
//! incoming arguments.

use schemars::JsonSchema;
use serde::Deserialize;

/// Arguments for the `index_repo` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct IndexRepoArgs {
    /// Absolute path to the repository to index.
    pub repo_path: String,
    /// Force a full re-index instead of incremental.
    #[serde(default)]
    pub force: bool,
}

/// Arguments for the `trace_symbol` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TraceSymbolArgs {
    /// The symbol name to trace (e.g. `parse_file`).
    pub symbol_name: String,
    /// Absolute path to the (already indexed) repository.
    pub repo_path: String,
    /// `"callers"`, `"callees"`, or `"both"` (default `"both"`).
    #[serde(default)]
    pub direction: Option<String>,
}

/// Arguments for the `find_context` tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindContextArgs {
    /// The task or question to gather context for.
    pub task: String,
    /// Absolute path to the (already indexed) repository.
    pub repo_path: String,
}
