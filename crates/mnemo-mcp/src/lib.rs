//! Mnemo MCP bridge.
//!
//! A stdio MCP server (via `rmcp`) that an MCP host (Claude Code, Cursor, …)
//! spawns. It holds no in-process state: each tool call is forwarded to the
//! running `mnemo-daemon` over the IPC client, auto-spawning a detached daemon
//! if none is up.
//!
//! ```text
//! MCP host --stdio MCP--> mnemo-mcp (this bridge) --IPC--> mnemo-daemon
//! ```
//!
//! Tools (M2.3): `index_repo`, `trace_symbol`, `find_context` (a basic,
//! unranked version). `impact_analysis` / `explain_pr` and `find_context`
//! ranking land in M3.

pub mod spawn;
pub mod tools;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::transport::io::stdio;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler, ServiceExt};
use serde_json::{json, Value};
use tools::{FindContextArgs, IndexRepoArgs, TraceSymbolArgs};

/// The MCP bridge: forwards tool calls to the daemon at `endpoint`.
#[derive(Clone)]
pub struct Bridge {
    endpoint: String,
    tool_router: ToolRouter<Bridge>,
}

#[tool_router]
impl Bridge {
    /// Create a bridge that forwards to the daemon at `endpoint`.
    pub fn new(endpoint: String) -> Self {
        Self {
            endpoint,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Index (or refresh) a repository so Mnemo can answer questions about it. \
                          Call this first for a repo, then use trace_symbol / find_context."
    )]
    async fn index_repo(
        &self,
        params: Parameters<IndexRepoArgs>,
    ) -> Result<CallToolResult, McpError> {
        ok_json(&self.do_index_repo(params.0).await?)
    }

    #[tool(description = "Trace callers and/or callees of a symbol by name in an indexed repo.")]
    async fn trace_symbol(
        &self,
        params: Parameters<TraceSymbolArgs>,
    ) -> Result<CallToolResult, McpError> {
        ok_json(&self.do_trace_symbol(params.0).await?)
    }

    #[tool(description = "Find a minimal set of relevant symbols for a task. \
                          MVP: keyword search over the indexed graph (no ranking yet).")]
    async fn find_context(
        &self,
        params: Parameters<FindContextArgs>,
    ) -> Result<CallToolResult, McpError> {
        ok_json(&self.do_find_context(params.0).await?)
    }
}

// Plain (macro-free) helpers — the unit-testable seam under each tool.
impl Bridge {
    /// Forward a JSON-RPC call to the daemon, mapping errors to MCP errors.
    async fn call(&self, method: &str, params: Value) -> Result<Value, McpError> {
        mnemo_daemon::client::call(&self.endpoint, method, params)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    /// Index a repo via `project.index` (attaches + caches as a side effect).
    pub async fn do_index_repo(&self, args: IndexRepoArgs) -> Result<Value, McpError> {
        self.call(
            "project.index",
            json!({ "path": args.repo_path, "force": args.force }),
        )
        .await
    }

    /// Trace callers/callees of a symbol via the cached graph.
    pub async fn do_trace_symbol(&self, args: TraceSymbolArgs) -> Result<Value, McpError> {
        let direction = args.direction.as_deref().unwrap_or("both");
        let mut out = json!({ "symbol": &args.symbol_name });
        if matches!(direction, "callers" | "both") {
            out["callers"] = self
                .call(
                    "query.callers",
                    json!({ "path": &args.repo_path, "query": &args.symbol_name }),
                )
                .await?;
        }
        if matches!(direction, "callees" | "both") {
            out["callees"] = self
                .call(
                    "query.callees",
                    json!({ "path": &args.repo_path, "query": &args.symbol_name }),
                )
                .await?;
        }
        Ok(out)
    }

    /// Basic context finder: union of keyword search hits over the task.
    pub async fn do_find_context(&self, args: FindContextArgs) -> Result<Value, McpError> {
        let mut seen = std::collections::BTreeSet::new();
        let mut items: Vec<Value> = Vec::new();
        for keyword in keywords(&args.task) {
            let hits = self
                .call(
                    "query.search",
                    json!({ "path": &args.repo_path, "query": keyword }),
                )
                .await?;
            if let Some(arr) = hits.as_array() {
                for hit in arr {
                    let key = hit["qualified_name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    if !key.is_empty() && seen.insert(key) {
                        items.push(hit.clone());
                    }
                }
            }
        }
        Ok(json!({
            "task": args.task,
            "summary": format!("{} candidate symbols matching task keywords", items.len()),
            "items": items,
            "note": "MVP: unranked keyword search over the indexed graph; \
                     ranking + token budget land in M3.",
        }))
    }
}

#[tool_handler]
impl ServerHandler for Bridge {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            instructions: Some(
                "Mnemo code intelligence. Call index_repo on a repository first, then \
                 trace_symbol or find_context. Backed by a local Mnemo daemon over IPC."
                    .to_string(),
            ),
            ..Default::default()
        }
    }
}

/// Wrap a JSON value as a successful MCP text result (pretty-printed).
fn ok_json(value: &Value) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

/// Split a task string into identifier-ish keywords (length ≥ 3).
fn keywords(task: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in task.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if word.len() >= 3 && !out.iter().any(|w| w == word) {
            out.push(word.to_string());
        }
    }
    out
}

/// Start the MCP bridge on stdio: ensure the daemon is up, then serve.
pub async fn serve() -> anyhow::Result<()> {
    // stdout is the MCP protocol channel — all logs must go to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let endpoint = mnemo_daemon::transport::default_endpoint();
    tracing::info!(endpoint, "mnemo-mcp bridge starting; ensuring daemon is up");
    spawn::ensure_daemon(&endpoint).await?;

    let service = Bridge::new(endpoint).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
