//! MCP server entrypoint for Mnemo.
//!
//! Starts an MCP-compliant server that exposes the following tools:
//! - `index_repo`
//! - `find_context`
//! - `trace_symbol`
//! - `impact_analysis`
//! - `explain_pr`
//!
//! Communication uses stdio transport by default.

use tracing_subscriber::EnvFilter;

pub mod tools;

/// Start the MCP server on stdio.
///
/// This is the main entrypoint for the `mnemo-mcp` binary.
/// It initializes tracing, creates the tool registry, and enters
/// the MCP request loop.
pub async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize structured logging.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tracing::info!("mnemo-mcp starting");

    // TODO: register tools with the rmcp server.
    // For now, this is a placeholder that keeps the binary compiling.
    tracing::warn!("MCP server transport not yet wired — serve() is a stub");

    Ok(())
}
