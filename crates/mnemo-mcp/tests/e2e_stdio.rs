//! End-to-end MCP host validation: spawn the real `mnemo-mcp` binary as
//! a subprocess, talk MCP/JSON-RPC over its stdio, and exercise the 3
//! tools end-to-end (index_repo → tools/list → find_context).
//!
//! This closes the M2.3 caveat: until now, bridge→daemon dispatch was unit-
//! tested via `bridge_forwards_to_daemon`, but the actual stdio MCP framing
//! had never been validated from outside the rmcp library. This test plays
//! the role of an MCP host (Claude Code, Cursor, …).
//!
//! `MNEMO_ENDPOINT` is set per-test to a unique value so the bridge — and
//! the daemon it auto-spawns — never collide with another test or with the
//! user's real daemon. The daemon is shut down explicitly at the end.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn copy_dir(src: &Path, dest: &Path) {
    std::fs::create_dir_all(dest).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dest.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
}

/// A unique IPC endpoint per test run (namespaced pipe on Windows, temp
/// socket on Unix). Mirrors `unique_endpoint` from the daemon's own
/// round-trip tests.
fn unique_endpoint() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let uniq = format!("{}-{}", std::process::id(), nanos);
    #[cfg(windows)]
    {
        format!("mnemo-e2e-{uniq}")
    }
    #[cfg(unix)]
    {
        std::env::temp_dir()
            .join(format!("mnemo-e2e-{uniq}.sock"))
            .to_string_lossy()
            .into_owned()
    }
}

/// Minimal MCP stdio client: line-delimited JSON-RPC 2.0 over the child's
/// stdin/stdout, matching the spec Claude Code / Cursor / other hosts use.
struct McpClient {
    proc: Child,
    next_id: u64,
}

impl McpClient {
    fn spawn(endpoint: &str) -> Self {
        let proc = Command::new(env!("CARGO_BIN_EXE_mnemo-mcp"))
            // Both bridge and the daemon it auto-spawns read MNEMO_ENDPOINT
            // (process env inheritance — see `mnemo-mcp::spawn::spawn_detached`).
            .env("MNEMO_ENDPOINT", endpoint)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Quiet the bridge's tracing output during tests; uncomment when
            // diagnosing a hang.
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn mnemo-mcp");
        Self { proc, next_id: 1 }
    }

    fn send_line(&mut self, msg: &Value) {
        let line = serde_json::to_string(msg).unwrap();
        let stdin = self.proc.stdin.as_mut().expect("stdin pipe");
        writeln!(stdin, "{line}").expect("write to bridge stdin");
        stdin.flush().expect("flush bridge stdin");
    }

    /// Send a JSON-RPC request, return the response with matching `id`.
    /// Notifications (no `id`) and out-of-order responses are silently skipped.
    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.send_line(&req);

        let stdout = self.proc.stdout.as_mut().expect("stdout pipe");
        let mut reader = BufReader::new(stdout);
        loop {
            let mut buf = String::new();
            let n = reader.read_line(&mut buf).expect("read mcp response line");
            assert!(n > 0, "bridge closed stdout before responding to id={id}");
            let line = buf.trim();
            if line.is_empty() {
                continue;
            }
            let resp: Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("non-JSON line from bridge: {line:?}: {e}"));
            if resp.get("id") == Some(&json!(id)) {
                return resp;
            }
            // Skip server-initiated notifications and stale responses.
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.proc.kill();
        let _ = self.proc.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_stdio_e2e_three_tools() {
    let endpoint = unique_endpoint();

    {
        let mut client = McpClient::spawn(&endpoint);

        // 1. MCP initialize handshake.
        let init = client.call(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "mnemo-e2e", "version": "0.1"}
            }),
        );
        assert!(
            init.get("result").is_some(),
            "initialize had no result: {init}"
        );

        // The spec requires the client to send this notification after init.
        client.send_line(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));

        // 2. tools/list — the three tools must be there.
        let tools = client.call("tools/list", json!({}));
        let names: Vec<&str> = tools["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list missing tools array: {tools}"))
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        for expected in &["index_repo", "trace_symbol", "find_context"] {
            assert!(
                names.contains(expected),
                "missing tool {expected}; got {names:?}"
            );
        }

        // 3. tools/call index_repo on a staged fixture.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("proj");
        copy_dir(&fixtures_root().join("basic_rust"), &repo);
        let repo_str = repo.to_string_lossy().into_owned();

        let index = client.call(
            "tools/call",
            json!({
                "name": "index_repo",
                "arguments": {"repo_path": &repo_str, "force": false}
            }),
        );
        let content = index["result"]["content"]
            .as_array()
            .unwrap_or_else(|| panic!("index_repo content missing: {index}"));
        assert!(!content.is_empty(), "index_repo content empty");

        // 4. tools/call find_context — the ranked Context Pack.
        let find = client.call(
            "tools/call",
            json!({
                "name": "find_context",
                "arguments": {"repo_path": &repo_str, "task": "square mul"}
            }),
        );
        let content = find["result"]["content"]
            .as_array()
            .unwrap_or_else(|| panic!("find_context content missing: {find}"));
        let text = content[0]["text"]
            .as_str()
            .expect("find_context text field");
        let pack: Value = serde_json::from_str(text)
            .unwrap_or_else(|e| panic!("find_context returned non-JSON text {text:?}: {e}"));
        let items = pack["items"]
            .as_array()
            .unwrap_or_else(|| panic!("pack has no items array: {pack}"));
        assert!(
            !items.is_empty(),
            "find_context returned empty items: {pack}"
        );
        // The two task tokens should both surface as candidates.
        let item_names: Vec<&str> = items.iter().filter_map(|i| i["name"].as_str()).collect();
        assert!(
            item_names.contains(&"square"),
            "expected `square` among items, got {item_names:?}"
        );
        assert!(
            item_names.contains(&"mul"),
            "expected `mul` among items, got {item_names:?}"
        );
    } // drop(client) → SIGKILLs the bridge subprocess.

    // The bridge auto-spawned a detached daemon; shut it down so it doesn't
    // leak between test runs. (On panic above this is skipped and the daemon
    // lingers; acceptable for a single dev box.)
    let _ = mnemo_daemon::client::call(&endpoint, "daemon.shutdown", json!({})).await;
}
