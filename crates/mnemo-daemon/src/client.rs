//! Minimal IPC client: connect, send one request, read one response.
//!
//! Each call uses a fresh connection (the CLI issues one request per command),
//! so no response-id multiplexing is needed.

use crate::protocol::{self, Request, Response};
use crate::transport;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Call `method` with `params` on the daemon at `endpoint`; return its result.
///
/// Surfaces a friendly error if the daemon isn't reachable, and propagates any
/// JSON-RPC error the daemon returns.
pub async fn call(endpoint: &str, method: &str, params: Value) -> anyhow::Result<Value> {
    let mut conn = transport::connect(endpoint).await.map_err(|e| {
        anyhow::anyhow!(
            "cannot reach the mnemo daemon at {endpoint}: {e}\n\
             start it with `mnemo daemon start --foreground`"
        )
    })?;

    let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let request = Request::new(id, method, params);
    let bytes = serde_json::to_vec(&request)?;
    protocol::write_frame(&mut conn, &bytes).await?;

    let frame = protocol::read_frame(&mut conn)
        .await?
        .ok_or_else(|| anyhow::anyhow!("daemon closed the connection without responding"))?;
    let response: Response = serde_json::from_slice(&frame)?;

    if let Some(error) = response.error {
        anyhow::bail!("daemon error {}: {}", error.code, error.message);
    }
    Ok(response.result.unwrap_or(Value::Null))
}
