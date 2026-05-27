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

    #[tool(
        description = "Find a minimal, ranked set of relevant symbols (a Context Pack) for a \
                          task, within a token budget. Anchors on task keywords, expands to \
                          callers/callees, and reflects unsaved edits via the overlay."
    )]
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

    /// Build a ranked Context Pack via the daemon's planner.
    pub async fn do_find_context(&self, args: FindContextArgs) -> Result<Value, McpError> {
        self.call(
            "context.find",
            json!({
                "path": args.repo_path,
                "task": args.task,
                "current_file": args.current_file,
                "changed_files": args.changed_files,
                "token_budget": args.token_budget,
            }),
        )
        .await
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
