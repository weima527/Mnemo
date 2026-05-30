//! Minimal IPC client: connect, send one request, read one response.
//!
//! Each call uses a fresh connection (the CLI issues one request per command),
//! so no response-id multiplexing is needed. Transient connection failures
//! (e.g. the OS pipe/socket briefly unavailable while the daemon re-arms its
//! listener under load) are retried; a JSON-RPC error from the daemon is not.

use crate::protocol::{self, Request, Response};
use crate::transport;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Max attempts for a transient connection failure. Empirically 15 is enough
/// to absorb the Windows named-pipe-listener-rearm race under heavy parallel
/// test load (six concurrent daemon-spawning tests across the workspace,
/// each with multi-`spawn_blocking` handlers — context.find / gc.run /
/// the M3.4 stdio e2e). Total retry budget: ~2.4s.
const MAX_ATTEMPTS: usize = 15;

/// A failed attempt: transient (worth retrying) vs fatal (return immediately).
enum CallError {
    Transient(anyhow::Error),
    Fatal(anyhow::Error),
}

/// Call `method` with `params` on the daemon at `endpoint`; return its result.
///
/// Retries transient connection failures with a short backoff. A daemon
/// JSON-RPC error (or a malformed response) is returned immediately.
pub async fn call(endpoint: &str, method: &str, params: Value) -> anyhow::Result<Value> {
    let mut last: Option<anyhow::Error> = None;
    for attempt in 0..MAX_ATTEMPTS {
        match attempt_call(endpoint, method, params.clone()).await {
            Ok(value) => return Ok(value),
            Err(CallError::Fatal(e)) => return Err(e),
            Err(CallError::Transient(e)) => {
                last = Some(e);
                tokio::time::sleep(Duration::from_millis(20 * (attempt as u64 + 1))).await;
            }
        }
    }
    Err(last
        .unwrap_or_else(|| anyhow::anyhow!("daemon unreachable"))
        .context(format!(
            "cannot reach the mnemo daemon at {endpoint} after {MAX_ATTEMPTS} attempts; \
             start it with `mnemo daemon start --foreground`"
        )))
}

/// One connect → send → receive round-trip.
async fn attempt_call(endpoint: &str, method: &str, params: Value) -> Result<Value, CallError> {
    let mut conn = transport::connect(endpoint)
        .await
        .map_err(|e| CallError::Transient(anyhow::anyhow!("connect failed: {e}")))?;

    let id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let request = Request::new(id, method, params);
    let bytes = serde_json::to_vec(&request).map_err(|e| CallError::Fatal(e.into()))?;
    protocol::write_frame(&mut conn, &bytes)
        .await
        .map_err(|e| CallError::Transient(anyhow::anyhow!("write failed: {e}")))?;

    let frame = protocol::read_frame(&mut conn)
        .await
        .map_err(|e| CallError::Transient(anyhow::anyhow!("read failed: {e}")))?
        .ok_or_else(|| {
            CallError::Transient(anyhow::anyhow!(
                "daemon closed the connection without responding"
            ))
        })?;

    let response: Response =
        serde_json::from_slice(&frame).map_err(|e| CallError::Fatal(e.into()))?;
    if let Some(error) = response.error {
        return Err(CallError::Fatal(anyhow::anyhow!(
            "daemon error {}: {}",
            error.code,
            error.message
        )));
    }
    Ok(response.result.unwrap_or(Value::Null))
}
