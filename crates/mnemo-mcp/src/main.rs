//! Binary entrypoint for the Mnemo MCP server.
//!
//! Starts the MCP server on stdio transport.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    mnemo_mcp::serve().await
}
