//! Working-tree file watcher → automatic overlay refresh.
//!
//! On project attach, the [`TenantManager`](super::TenantManager) starts an
//! [`OverlayWatcher`] that watches the project's canonical root via
//! `notify-debouncer-mini` (cross-platform: inotify on Linux, FSEvents on
//! macOS, ReadDirectoryChangesW on Windows). Bursts of file events within a
//! ~2-second debounce window are batched into a single flush that re-reads
//! the changed files and calls [`ProjectContext::set_overlay`] — so live
//! edits reflect in queries without explicit `overlay.set` IPC calls. The
//! watcher never writes the DB (DESIGN §12 #7).
//!
//! Dropped on detach / eviction → stops the platform watcher thread + aborts
//! the forwarder tokio task.

use crate::context::ProjectContext;
use mnemo_core::{CoreError, Language};
use mnemo_index::overlay::OverlayFile;
use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

/// How long a quiet period before flushing accumulated dirty files.
const DEBOUNCE_MS: u64 = 2_000;

/// Top-level directory names skipped by the watcher (parallels the indexer's
/// `is_ignored` list in `mnemo-index::lib`).
const DEFAULT_IGNORE: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    ".cache",
    ".venv",
    "venv",
    "__pycache__",
];

/// A live working-tree watcher attached to one [`ProjectContext`]. Drop it
/// to stop watching and to abort the forwarder task.
pub struct OverlayWatcher {
    /// Owns the platform watcher thread; dropping releases the inotify fd
    /// (or the named pipe / FSEvents stream).
    _debouncer: Debouncer<notify::RecommendedWatcher>,
    /// Forwarder task that converts debounced batches into `set_overlay`
    /// calls. Aborted explicitly in [`Drop`].
    task: JoinHandle<()>,
}

impl OverlayWatcher {
    /// Start watching `ctx.canonical_path()` recursively. Any failure to bind
    /// the watcher (e.g. permission denied on the path) returns the error;
    /// callers in the tenant layer log-and-skip so attach still succeeds
    /// without the live-edit feature.
    pub fn start(ctx: Arc<ProjectContext>) -> Result<Self, CoreError> {
        let root = ctx.canonical_path().to_path_buf();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<PathBuf>>();

        // Producer: the notify thread fires this callback every DEBOUNCE_MS
        // with the batch of distinct paths that changed during the window.
        let mut debouncer = new_debouncer(
            Duration::from_millis(DEBOUNCE_MS),
            move |result: DebounceEventResult| {
                if let Ok(events) = result {
                    let paths: Vec<PathBuf> = events.into_iter().map(|e| e.path).collect();
                    if !paths.is_empty() {
                        // Receiver may have been dropped (watcher stopping);
                        // silently drop the batch in that case.
                        let _ = tx.send(paths);
                    }
                }
            },
        )
        .map_err(notify_err)?;
        debouncer
            .watcher()
            .watch(&root, RecursiveMode::Recursive)
            .map_err(notify_err)?;

        // Consumer: accumulate a per-rel-path dirty set, calling set_overlay
        // with the full current set on every flush. (set_overlay replaces, so
        // we accumulate state ourselves to preserve earlier edits.)
        let task = tokio::spawn(async move {
            let mut dirty: HashMap<String, OverlayFile> = HashMap::new();
            while let Some(paths) = rx.recv().await {
                let mut changed = false;
                for path in paths {
                    if let Some(file) = path_to_overlay(&root, &path) {
                        dirty.insert(file.rel_path.clone(), file);
                        changed = true;
                    }
                }
                if changed {
                    let files: Vec<OverlayFile> = dirty.values().cloned().collect();
                    if let Err(e) = ctx.set_overlay(files).await {
                        tracing::warn!(error = %e, "overlay watcher: set_overlay failed");
                    }
                }
            }
        });

        Ok(Self {
            _debouncer: debouncer,
            task,
        })
    }
}

impl Drop for OverlayWatcher {
    fn drop(&mut self) {
        // Abort the forwarder. The debouncer field is dropped right after,
        // which closes the platform watcher and drops the sender — so any
        // in-flight notify callback becomes a no-op.
        self.task.abort();
    }
}

fn notify_err(e: notify::Error) -> CoreError {
    CoreError::Io(io::Error::other(format!("watcher: {e}")))
}

/// Whether `rel` (a forward-slash, repo-relative path) lives under one of the
/// default ignored top-level directories.
fn is_ignored(rel: &str) -> bool {
    if let Some(first) = rel.split('/').next() {
        DEFAULT_IGNORE.contains(&first)
    } else {
        false
    }
}

/// Build the [`OverlayFile`] for an event path under `root`, or `None` to
/// skip (root itself, unsupported extension, ignored directory).
fn path_to_overlay(root: &Path, path: &Path) -> Option<OverlayFile> {
    if path == root {
        return None;
    }
    let rel = path
        .strip_prefix(root)
        .ok()?
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/");
    if rel.is_empty() || is_ignored(&rel) {
        return None;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    Language::from_extension(ext)?;
    // `read_to_string` failing typically means the file was deleted — record
    // it as content=None so the overlay parser drops it from the graph.
    let content = std::fs::read_to_string(path).ok();
    Some(OverlayFile {
        rel_path: rel,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_default_directories() {
        assert!(is_ignored(".git/HEAD"));
        assert!(is_ignored("target/debug/foo"));
        assert!(is_ignored("node_modules/pkg/index.js"));
        assert!(!is_ignored("src/main.rs"));
        assert!(!is_ignored("crates/mnemo-core/src/lib.rs"));
    }

    #[test]
    fn path_to_overlay_recognizes_rust_and_skips_unknown() {
        let root = Path::new("/repo");
        // Unsupported extension: skipped.
        assert!(path_to_overlay(root, Path::new("/repo/README.md")).is_none());
        // Ignored directory: skipped.
        assert!(path_to_overlay(root, Path::new("/repo/target/x.rs")).is_none());
        // root itself: skipped.
        assert!(path_to_overlay(root, root).is_none());
        // Supported extension under a non-ignored path is accepted (content
        // None because the path doesn't exist on disk — but the routing is
        // still correct).
        let of = path_to_overlay(root, Path::new("/repo/src/main.rs")).expect("recognised");
        assert_eq!(of.rel_path, "src/main.rs");
        assert!(of.content.is_none());
    }

    #[tokio::test]
    async fn watcher_starts_on_a_tempdir() {
        // Spawn an OverlayWatcher against a temp dir and immediately drop it.
        // The point is that creating it (a) compiles, (b) doesn't panic, and
        // (c) the Drop path tears everything down cleanly.
        let tmp = tempfile::tempdir().unwrap();
        // Build a minimal ProjectContext via the public attach surface so we
        // exercise the real wiring. resolve_project_id needs the dir to
        // exist; we use the tempdir root.
        // (We can't easily construct a `ProjectContext` from scratch outside
        // its crate; instead, use TenantManager.)
        let mgr = crate::TenantManager::new(crate::TenantConfig::default());
        let ctx = mgr.attach(tmp.path()).await.expect("attach tempdir");
        let w = OverlayWatcher::start(ctx).expect("watcher start");
        drop(w);
    }
}
