//! Binary entrypoint for the Mnemo MCP bridge (stdio transport).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mnemo_mcp::serve().await
}
