//! JSON-RPC 2.0 envelopes, method parameter/result types, and length-prefixed
//! framing for the Mnemo daemon IPC.
//!
//! Wire format: each message is a 4-byte big-endian length prefix followed by a
//! JSON-RPC 2.0 payload of that many bytes.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum accepted frame size (64 MiB) — rejects garbage/oversized prefixes.
const MAX_FRAME: usize = 64 * 1024 * 1024;

/// JSON-RPC error codes used by the daemon.
pub mod codes {
    /// Malformed JSON in the request frame.
    pub const PARSE_ERROR: i64 = -32700;
    /// Unknown method.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// Params did not match the method's schema.
    pub const INVALID_PARAMS: i64 = -32602;
    /// Internal error (a `CoreError` from the handler).
    pub const INTERNAL_ERROR: i64 = -32603;
}

/// A JSON-RPC 2.0 request.
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    /// Build a request with the standard `"2.0"` version tag.
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    /// A success response.
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// An error response.
    pub fn err(id: Value, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// Build an error with no `data`.
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Method parameter / result types
// ---------------------------------------------------------------------------

/// Params for methods identifying a project by path (`project.attach`).
#[derive(Debug, Serialize, Deserialize)]
pub struct PathParams {
    pub path: String,
}

/// Params for `project.detach`.
#[derive(Debug, Serialize, Deserialize)]
pub struct DetachParams {
    pub project_id: String,
}

/// Params for `project.index`.
#[derive(Debug, Serialize, Deserialize)]
pub struct IndexParams {
    pub path: String,
    #[serde(default)]
    pub force: bool,
}

/// Params for the `query.*` methods (`query` is the name or search pattern).
#[derive(Debug, Serialize, Deserialize)]
pub struct QueryParams {
    pub path: String,
    pub query: String,
}

/// Summary of one project (in `project.attach`, `project.list`, status).
#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub project_id: String,
    pub canonical_path: String,
    pub symbol_count: usize,
}

/// Result of `project.index`.
#[derive(Debug, Serialize, Deserialize)]
pub struct IndexInfo {
    pub snapshot: i64,
    pub file_count: usize,
    pub symbol_count: usize,
    pub edge_count: usize,
}

/// Result of `daemon.status`.
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub active_projects: usize,
    pub max_active_projects: usize,
    pub uptime_secs: u64,
    pub projects: Vec<ProjectInfo>,
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// Write a length-prefixed frame (4-byte big-endian length + payload).
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> std::io::Result<()> {
    let len = u32::try_from(payload.len()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large to send")
    })?;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(payload).await?;
    w.flush().await?;
    Ok(())
}

/// Read a length-prefixed frame. Returns `None` on a clean EOF at a boundary.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "incoming frame exceeds maximum size",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(Some(buf))
}
