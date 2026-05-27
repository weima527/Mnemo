//! Auto-spawn a detached `mnemo-daemon` when none is reachable.

use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Ensure a daemon is reachable at `endpoint`, spawning a detached one if not.
pub async fn ensure_daemon(endpoint: &str) -> anyhow::Result<()> {
    if daemon_up(endpoint).await {
        return Ok(());
    }

    let bin = daemon_binary()?;
    if !bin.exists() {
        anyhow::bail!(
            "mnemo-daemon binary not found at {} (expected next to mnemo-mcp)",
            bin.display()
        );
    }
    tracing::info!(bin = %bin.display(), "daemon not running; spawning detached");
    spawn_detached(&bin)?;

    // Poll until the daemon binds its socket.
    for _ in 0..100 {
        if daemon_up(endpoint).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!("spawned mnemo-daemon but it did not become ready in time");
}

/// Whether the daemon answers a status request.
async fn daemon_up(endpoint: &str) -> bool {
    mnemo_daemon::client::call(endpoint, "daemon.status", json!({}))
        .await
        .is_ok()
}

/// Locate the `mnemo-daemon` binary next to the current executable (both bins
/// build/install to the same directory).
fn daemon_binary() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("current executable has no parent directory"))?;
    let name = if cfg!(windows) {
        "mnemo-daemon.exe"
    } else {
        "mnemo-daemon"
    };
    Ok(dir.join(name))
}

/// Spawn `bin` as a detached, null-piped background process.
fn spawn_detached(bin: &Path) -> std::io::Result<()> {
    let mut cmd = Command::new(bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NO_WINDOW: no console, independent lifetime.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }

    // Unix: null-piped spawn suffices for the MVP; full session detach (setsid)
    // is a later refinement (avoided here to skip a libc dependency).
    cmd.spawn()?;
    Ok(())
}
