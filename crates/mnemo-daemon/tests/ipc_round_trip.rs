//! End-to-end IPC round-trip over the real cross-platform transport
//! (Unix domain socket on Unix, Windows named pipe on Windows).

use mnemo_daemon::client::call;
use mnemo_tenant::{TenantConfig, TenantManager};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A unique endpoint per test run: a namespaced pipe key on Windows, a temp
/// filesystem socket path on Unix.
fn unique_endpoint() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let uniq = format!("{}-{}", std::process::id(), nanos);
    #[cfg(windows)]
    {
        format!("mnemo-test-{uniq}")
    }
    #[cfg(unix)]
    {
        std::env::temp_dir()
            .join(format!("mnemo-test-{uniq}.sock"))
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
async fn ipc_round_trip() {
    let endpoint = unique_endpoint();
    let manager = Arc::new(TenantManager::new(TenantConfig::default()));
    let server = {
        let endpoint = endpoint.clone();
        tokio::spawn(async move { mnemo_daemon::run(&endpoint, manager).await })
    };

    // Wait for the listener to come up.
    let mut up = false;
    for _ in 0..100 {
        if call(&endpoint, "daemon.status", json!({})).await.is_ok() {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(up, "daemon did not start listening");

    // Initially no projects are active.
    let status = call(&endpoint, "daemon.status", json!({})).await.unwrap();
    assert_eq!(status["active_projects"], 0);

    // Stage the basic_rust fixture in a tempdir and attach + index it over IPC.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("proj");
    copy_dir(&fixtures_root().join("basic_rust"), &repo);
    let repo_str = repo.to_string_lossy().into_owned();

    call(&endpoint, "project.attach", json!({ "path": repo_str }))
        .await
        .unwrap();
    call(
        &endpoint,
        "project.index",
        json!({ "path": repo_str, "force": false }),
    )
    .await
    .unwrap();

    // One project active after attach.
    let status = call(&endpoint, "daemon.status", json!({})).await.unwrap();
    assert_eq!(status["active_projects"], 1);

    // square() calls mul() (same file) → mul has a caller, served from cache.
    let callers = call(
        &endpoint,
        "query.callers",
        json!({ "path": repo_str, "query": "mul" }),
    )
    .await
    .unwrap();
    assert!(
        callers.as_array().is_some_and(|a| !a.is_empty()),
        "expected callers for `mul` over IPC, got {callers}"
    );

    // search returns the fixture symbols.
    let hits = call(
        &endpoint,
        "query.search",
        json!({ "path": repo_str, "query": "add" }),
    )
    .await
    .unwrap();
    assert!(
        hits.as_array().is_some_and(|a| !a.is_empty()),
        "expected search hits for `add`, got {hits}"
    );

    // Graceful shutdown ends the server task.
    call(&endpoint, "daemon.shutdown", json!({})).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
}

/// A modified `math.rs` that adds a `triple` function.
const MODIFIED_MATH: &str = "\
pub fn add(a: i64, b: i64) -> i64 { a + b }
pub fn mul(a: i64, b: i64) -> i64 { a * b }
pub fn square(n: i64) -> i64 { mul(n, n) }
pub fn triple(n: i64) -> i64 { add(n, add(n, n)) }
";

#[tokio::test]
async fn overlay_round_trip() {
    let endpoint = unique_endpoint();
    let manager = Arc::new(TenantManager::new(TenantConfig::default()));
    let server = {
        let endpoint = endpoint.clone();
        tokio::spawn(async move { mnemo_daemon::run(&endpoint, manager).await })
    };
    let mut up = false;
    for _ in 0..100 {
        if call(&endpoint, "daemon.status", json!({})).await.is_ok() {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(up, "daemon did not start listening");

    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("proj");
    copy_dir(&fixtures_root().join("basic_rust"), &repo);
    let repo_str = repo.to_string_lossy().into_owned();

    call(
        &endpoint,
        "project.index",
        json!({ "path": repo_str, "force": false }),
    )
    .await
    .unwrap();

    // `triple` is not in the base index.
    let base = call(
        &endpoint,
        "query.search",
        json!({ "path": repo_str, "query": "triple" }),
    )
    .await
    .unwrap();
    assert!(
        base.as_array().is_some_and(|a| a.is_empty()),
        "base should lack `triple`, got {base}"
    );

    // Overlay a modified math.rs; `triple` becomes visible over IPC.
    call(
        &endpoint,
        "overlay.set",
        json!({ "path": repo_str, "files": [{ "rel_path": "src/math.rs", "content": MODIFIED_MATH }] }),
    )
    .await
    .unwrap();
    let overlaid = call(
        &endpoint,
        "query.search",
        json!({ "path": repo_str, "query": "triple" }),
    )
    .await
    .unwrap();
    assert!(
        overlaid.as_array().is_some_and(|a| !a.is_empty()),
        "overlay `triple` should be visible over IPC, got {overlaid}"
    );

    // Clearing the overlay falls back to the base graph.
    call(&endpoint, "overlay.clear", json!({ "path": repo_str }))
        .await
        .unwrap();
    let cleared = call(
        &endpoint,
        "query.search",
        json!({ "path": repo_str, "query": "triple" }),
    )
    .await
    .unwrap();
    assert!(
        cleared.as_array().is_some_and(|a| a.is_empty()),
        "after clear, `triple` should be gone, got {cleared}"
    );

    call(&endpoint, "daemon.shutdown", json!({})).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
}
