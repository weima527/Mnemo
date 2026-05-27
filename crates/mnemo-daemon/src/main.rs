//! Standalone Mnemo daemon binary. Runs the server on the default endpoint
//! until `daemon.shutdown` or Ctrl-C.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    mnemo_daemon::run_default().await
}
