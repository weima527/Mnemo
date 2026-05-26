//! Per-project in-memory state held by the daemon.
//!
//! A [`ProjectContext`] caches the project's hydrated [`SymbolGraph`] in an
//! `ArcSwap` for lock-free reads. The DB is opened on demand for the rare
//! operations that touch it (hydration, re-index) — there is no long-lived
//! `Connection` shared across threads — and every such op runs on
//! `tokio::task::spawn_blocking`.

use arc_swap::ArcSwap;
use mnemo_core::{CoreError, ProjectId};
use mnemo_graph::SymbolGraph;
use mnemo_store::dao::snapshot as snapshot_dao;
use mnemo_store::open_database;
use mnemo_store::paths::{ensure_home_layout, project_db, resolve_project_id};
use mnemo_store::registry::{open_registry, upsert_project};
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
    graph: ArcSwap<SymbolGraph>,
    stats: Stats,
}

impl ProjectContext {
    /// Resolve `path`, ensure its DB exists and is registered, and hydrate the
    /// graph at the latest snapshot (empty if the project was never indexed).
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

        let graph = match snapshot_dao::latest(&conn)? {
            Some(snapshot) => mnemo_index::query::hydrate(&conn, snapshot)?,
            None => SymbolGraph::default(),
        };

        Ok(Arc::new(Self {
            project_id,
            canonical_path: canonical,
            db_path,
            graph: ArcSwap::from_pointee(graph),
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

    /// Lock-free snapshot of the current symbol graph.
    pub fn graph(&self) -> Arc<SymbolGraph> {
        self.graph.load_full()
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

    /// Re-load the graph from the DB at the latest snapshot and atomically swap
    /// it in. Call after a re-index so cached queries see fresh data.
    pub async fn rehydrate(&self) -> Result<(), CoreError> {
        let db_path = self.db_path.clone();
        let graph = tokio::task::spawn_blocking(move || -> Result<SymbolGraph, CoreError> {
            let conn = open_database(&db_path)?;
            match snapshot_dao::latest(&conn)? {
                Some(snapshot) => mnemo_index::query::hydrate(&conn, snapshot),
                None => Ok(SymbolGraph::default()),
            }
        })
        .await
        .expect("tenant rehydrate task panicked")?;
        self.graph.store(Arc::new(graph));
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
