//! MCP tool implementations.
//!
//! Each public function corresponds to an MCP tool exposed by the server.
//! All tools are read-only unless explicitly noted.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tool input / output schemas
// ---------------------------------------------------------------------------

/// Input for the `index_repo` tool.
#[derive(Debug, Deserialize)]
pub struct IndexRepoInput {
    /// Path to the repository to index.
    pub repo_path: String,
    /// Optional language filter.
    pub language: Option<String>,
    /// If true, discard existing index and rebuild from scratch.
    #[serde(default)]
    pub force: bool,
}

/// Output from the `index_repo` tool.
#[derive(Debug, Serialize)]
pub struct IndexRepoOutput {
    pub file_count: usize,
    pub symbol_count: usize,
    pub edge_count: usize,
    pub elapsed_ms: u64,
    pub index_version: u32,
}

/// Input for the `find_context` tool.
#[derive(Debug, Deserialize)]
pub struct FindContextInput {
    /// Natural-language task description.
    pub task: String,
    /// Path to the repository.
    pub repo_path: String,
    /// Optional: current file the user is editing.
    pub current_file: Option<String>,
    /// Token budget for the Context Pack output.
    #[serde(default = "default_token_budget")]
    pub token_budget: u32,
    /// Optional: list of files known to be changed.
    pub changed_files: Option<Vec<String>>,
}

fn default_token_budget() -> u32 {
    5000
}

/// Output from the `find_context` tool — a Context Pack.
#[derive(Debug, Serialize)]
pub struct FindContextOutput {
    pub task: String,
    pub budget: BudgetInfo,
    pub summary: String,
    pub items: Vec<ContextPackItem>,
    pub omitted: Vec<OmittedItem>,
}

#[derive(Debug, Serialize)]
pub struct BudgetInfo {
    pub requested_tokens: u32,
    pub estimated_tokens: u32,
}

#[derive(Debug, Serialize)]
pub struct ContextPackItem {
    pub kind: String,
    pub name: String,
    pub file: String,
    pub reason: String,
    pub score: f64,
}

#[derive(Debug, Serialize)]
pub struct OmittedItem {
    pub file: String,
    pub reason: String,
}

/// Input for the `trace_symbol` tool.
#[derive(Debug, Deserialize)]
pub struct TraceSymbolInput {
    pub symbol_name: String,
    pub repo_path: String,
    pub file_path: Option<String>,
    pub direction: Option<String>,
    pub max_depth: Option<u32>,
}

/// Input for the `impact_analysis` tool.
#[derive(Debug, Deserialize)]
pub struct ImpactAnalysisInput {
    pub file_path: Option<String>,
    pub symbol: Option<String>,
    pub repo_path: String,
    pub diff: Option<String>,
    pub max_depth: Option<u32>,
}

/// Input for the `explain_pr` tool.
#[derive(Debug, Deserialize)]
pub struct ExplainPrInput {
    pub base_ref: String,
    pub head_ref: String,
    pub repo_path: String,
    #[serde(default = "default_token_budget")]
    pub token_budget: u32,
}
