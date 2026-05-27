//! The bridge forwards MCP tool calls to a real daemon over IPC.
//!
//! Drives the macro-free `do_*` helpers (the testable seam under each `#[tool]`)
//! against an in-process daemon on a unique endpoint — exercising the full
//! bridge -> IPC -> daemon -> cached-graph path without the stdio/MCP layer.

use mnemo_mcp::tools::{FindContextArgs, IndexRepoArgs, TraceSymbolArgs};
use mnemo_mcp::Bridge;
use mnemo_tenant::{TenantConfig, TenantManager};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn unique_endpoint() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let uniq = format!("{}-{}", std::process::id(), nanos);
    #[cfg(windows)]
    {
        format!("mnemo-mcp-test-{uniq}")
    }
    #[cfg(unix)]
    {
        std::env::temp_dir()
            .join(format!("mnemo-mcp-test-{uniq}.sock"))
            .to_string_lossy()
            .into_owned()
    }
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
}

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

#[tokio::test]
async fn bridge_forwards_to_daemon() {
    let endpoint = unique_endpoint();
    let manager = Arc::new(TenantManager::new(TenantConfig::default()));
    let server = {
        let endpoint = endpoint.clone();
        tokio::spawn(async move { mnemo_daemon::run(&endpoint, manager).await })
    };

    // Wait for the daemon to come up.
    let mut up = false;
    for _ in 0..100 {
        if mnemo_daemon::client::call(&endpoint, "daemon.status", json!({}))
            .await
            .is_ok()
        {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(up, "daemon did not start");

    let bridge = Bridge::new(endpoint.clone());

    // Stage + index the fixture through the bridge.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("proj");
    copy_dir(&fixtures_root().join("basic_rust"), &repo);
    let repo_str = repo.to_string_lossy().into_owned();

    let idx = bridge
        .do_index_repo(IndexRepoArgs {
            repo_path: repo_str.clone(),
            force: false,
        })
        .await
        .unwrap();
    assert!(
        idx["symbol_count"].as_u64().unwrap_or(0) >= 1,
        "expected symbols indexed via the bridge, got {idx}"
    );

    // trace_symbol: mul is called by square (same file).
    let trace = bridge
        .do_trace_symbol(TraceSymbolArgs {
            symbol_name: "mul".to_string(),
            repo_path: repo_str.clone(),
            direction: None,
        })
        .await
        .unwrap();
    assert!(
        trace["callers"].as_array().is_some_and(|a| !a.is_empty()),
        "expected callers for `mul`, got {trace}"
    );

    // find_context: keyword search over the task returns items.
    let ctx = bridge
        .do_find_context(FindContextArgs {
            task: "mul add".to_string(),
            repo_path: repo_str.clone(),
        })
        .await
        .unwrap();
    assert!(
        ctx["items"].as_array().is_some_and(|a| !a.is_empty()),
        "expected find_context items, got {ctx}"
    );

    // Shut the in-process daemon down.
    let _ = mnemo_daemon::client::call(&endpoint, "daemon.shutdown", json!({})).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
}
