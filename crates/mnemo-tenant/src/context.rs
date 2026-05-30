//! Per-project in-memory state held by the daemon.
//!
//! A [`ProjectContext`] caches the project's hydrated [`SymbolGraph`] in an
//! `ArcSwap` for lock-free reads. The DB is opened on demand for the rare
//! operations that touch it (hydration, re-index, overlay rebuild) — there is no
//! long-lived `Connection` shared across threads — and every such op runs on
//! `tokio::task::spawn_blocking`.
//!
//! On top of the base (last-indexed snapshot) graph, an optional **working-tree
//! overlay** holds uncommitted edits re-parsed in memory; `graph()` is
//! overlay-first. The overlay never writes the DB (DESIGN §6.3).

use arc_swap::{ArcSwap, ArcSwapOption};
use mnemo_core::{CoreError, ProjectId};
use mnemo_graph::SymbolGraph;
use mnemo_index::overlay::{hydrate_with_overlay, OverlayFile};
use mnemo_store::dao::gc::{self as gc_dao, GcPolicy, GcReport};
use mnemo_store::dao::snapshot as snapshot_dao;
use mnemo_store::open_database;
use mnemo_store::paths::{ensure_home_layout, project_db, resolve_project_id};
use mnemo_store::registry::{open_registry, upsert_project};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Lightweight per-project counters for observability.
#[derive(Debug, Default)]
pub struct Stats {
    /// Unix seconds of the last access (attach / get / query).
    pub last_accessed: AtomicI64,
    /// Number of queries served from this context.
    pub query_count: AtomicU64,
}

/// In-memory state for one attached project.
#[derive(Debug)]
pub struct ProjectContext {
    project_id: ProjectId,
    canonical_path: PathBuf,
    db_path: PathBuf,
    /// Graph at the latest indexed snapshot (DB-backed).
    base: ArcSwap<SymbolGraph>,
    /// Merged base+overlay graph; `None` when no working-tree overlay is set.
    overlay: ArcSwapOption<SymbolGraph>,
    /// The changed files currently overlaid (kept so the overlay can be rebuilt
    /// after a re-index).
    overlay_files: Mutex<Vec<OverlayFile>>,
    stats: Stats,
}

impl ProjectContext {
    /// Resolve `path`, ensure its DB exists and is registered, and hydrate the
    /// base graph at the latest snapshot (empty if the project was never indexed).
    pub(crate) async fn build(path: &Path) -> Result<Arc<Self>, CoreError> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || Self::build_blocking(&path))
            .await
            .expect("tenant build task panicked")
    }

    fn build_blocking(path: &Path) -> Result<Arc<Self>, CoreError> {
        let (project_id, canonical) = resolve_project_id(path)?;
        ensure_home_layout()?;

        let db_path = project_db(project_id);
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(CoreError::Io)?;
        }
        let conn = open_database(&db_path)?;

        // Register / refresh the project in the global registry.
        let registry = open_registry()?;
        let display_name = canonical
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed");
        upsert_project(
            &registry,
            project_id,
            &canonical.to_string_lossy(),
            display_name,
        )?;

        let base = match snapshot_dao::latest(&conn)? {
            Some(snapshot) => mnemo_index::query::hydrate(&conn, snapshot)?,
            None => SymbolGraph::default(),
        };

        Ok(Arc::new(Self {
            project_id,
            canonical_path: canonical,
            db_path,
            base: ArcSwap::from_pointee(base),
            overlay: ArcSwapOption::empty(),
            overlay_files: Mutex::new(Vec::new()),
            stats: Stats::default(),
        }))
    }

    /// Stable identity of the project.
    pub fn project_id(&self) -> ProjectId {
        self.project_id
    }

    /// Canonical absolute path of the project root.
    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    /// Absolute path to the per-project index DB (under `~/.mnemo/`).
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Lock-free snapshot of the active graph: the overlay if one is set,
    /// otherwise the base (indexed) graph.
    pub fn graph(&self) -> Arc<SymbolGraph> {
        self.overlay
            .load_full()
            .unwrap_or_else(|| self.base.load_full())
    }

    /// Whether a working-tree overlay is currently active.
    pub fn has_overlay(&self) -> bool {
        self.overlay.load().is_some()
    }

    /// Unix seconds of the last access.
    pub fn last_accessed(&self) -> i64 {
        self.stats.last_accessed.load(Ordering::Relaxed)
    }

    /// Number of queries served from this context.
    pub fn query_count(&self) -> u64 {
        self.stats.query_count.load(Ordering::Relaxed)
    }

    /// Record an access (updates `last_accessed`).
    pub fn touch(&self) {
        self.stats
            .last_accessed
            .store(now_secs(), Ordering::Relaxed);
    }

    /// Record a served query (bumps `query_count` and `last_accessed`).
    pub fn record_query(&self) {
        self.stats.query_count.fetch_add(1, Ordering::Relaxed);
        self.touch();
    }

    /// Set (replace) the working-tree overlay from `files` and rebuild the
    /// merged graph in memory. Never writes the DB.
    pub async fn set_overlay(&self, files: Vec<OverlayFile>) -> Result<(), CoreError> {
        let merged = self.build_overlay(&files).await?;
        *self.overlay_files.lock() = files;
        self.overlay.store(Some(Arc::new(merged)));
        Ok(())
    }

    /// Drop the working-tree overlay; queries fall back to the base graph.
    pub fn clear_overlay(&self) {
        self.overlay.store(None);
        self.overlay_files.lock().clear();
    }

    /// Build the merged base+overlay graph for `files` (read-only on the DB).
    async fn build_overlay(&self, files: &[OverlayFile]) -> Result<SymbolGraph, CoreError> {
        let project_id = self.project_id;
        let db_path = self.db_path.clone();
        let files = files.to_vec();
        tokio::task::spawn_blocking(move || -> Result<SymbolGraph, CoreError> {
            let conn = open_database(&db_path)?;
            match snapshot_dao::latest(&conn)? {
                Some(snapshot) => hydrate_with_overlay(&conn, snapshot, project_id, &files),
                None => Ok(SymbolGraph::default()),
            }
        })
        .await
        .expect("tenant overlay task panicked")
    }

    /// Run a single GC pass against this project's DB (DESIGN §8.3, MVP
    /// simplification): clean up old anonymous / working_tree snapshots.
    /// Commit / manual / filehash snapshots are never touched.
    pub async fn gc(&self, policy: GcPolicy) -> Result<GcReport, CoreError> {
        let db_path = self.db_path.clone();
        tokio::task::spawn_blocking(move || -> Result<GcReport, CoreError> {
            let mut conn = open_database(&db_path)?;
            gc_dao::run(&mut conn, &policy)
        })
        .await
        .expect("tenant gc task panicked")
    }

    /// Re-load the base graph from the DB at the latest snapshot, and rebuild the
    /// overlay (if any) on top of it. Call after a re-index.
    pub async fn rehydrate(&self) -> Result<(), CoreError> {
        let db_path = self.db_path.clone();
        let base = tokio::task::spawn_blocking(move || -> Result<SymbolGraph, CoreError> {
            let conn = open_database(&db_path)?;
            match snapshot_dao::latest(&conn)? {
                Some(snapshot) => mnemo_index::query::hydrate(&conn, snapshot),
                None => Ok(SymbolGraph::default()),
            }
        })
        .await
        .expect("tenant rehydrate task panicked")?;
        self.base.store(Arc::new(base));

        let files = self.overlay_files.lock().clone();
        if !files.is_empty() {
            self.set_overlay(files).await?;
        }
        Ok(())
    }
}

/// Current unix time in seconds (0 on the impossible pre-epoch case).
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
