//! The multi-project manager: attach / get / detach with LRU eviction.

use crate::context::ProjectContext;
use dashmap::DashMap;
use lru::LruCache;
use mnemo_core::{CoreError, ProjectId};
use mnemo_store::paths::resolve_project_id;
use mnemo_store::registry::{archive_project, open_registry};
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Configuration for the [`TenantManager`].
#[derive(Debug, Clone)]
pub struct TenantConfig {
    /// Maximum number of projects kept hydrated in memory at once.
    pub max_active_projects: usize,
}

impl Default for TenantConfig {
    fn default() -> Self {
        Self {
            max_active_projects: 5,
        }
    }
}

/// A summary of one active project (for `list_active`).
#[derive(Debug, Clone)]
pub struct ProjectSummary {
    /// Stable project identity.
    pub project_id: ProjectId,
    /// Canonical absolute path of the project root.
    pub canonical_path: PathBuf,
    /// Symbols currently held in the in-memory graph.
    pub symbol_count: usize,
    /// Unix seconds of the last access.
    pub last_accessed: i64,
}

/// Holds several projects' in-memory state, evicting the least-recently-used
/// project when over capacity.
///
/// Eviction frees memory only — it never deletes a DB file or registry row
/// (DESIGN §4.4). `detach` additionally marks the registry row `archived`.
pub struct TenantManager {
    projects: DashMap<ProjectId, Arc<ProjectContext>>,
    /// Recency tracking only; the contexts live in `projects`.
    lru: Mutex<LruCache<ProjectId, ()>>,
    config: TenantConfig,
}

impl TenantManager {
    /// Create a manager with the given config.
    pub fn new(config: TenantConfig) -> Self {
        // `max(1)` guarantees a non-zero capacity even on a misconfigured 0.
        let cap = NonZeroUsize::new(config.max_active_projects.max(1))
            .expect("max(1) is always non-zero");
        Self {
            projects: DashMap::new(),
            lru: Mutex::new(LruCache::new(cap)),
            config,
        }
    }

    /// Attach a project by path: hydrate and cache it (or bump it if already
    /// cached), evicting the least-recently-used project if over capacity.
    pub async fn attach(&self, path: &Path) -> Result<Arc<ProjectContext>, CoreError> {
        // Resolve the id off the async thread (canonicalize hits disk).
        let path_buf = path.to_path_buf();
        let (project_id, _canonical) =
            tokio::task::spawn_blocking(move || resolve_project_id(&path_buf))
                .await
                .expect("resolve_project_id task panicked")?;

        // Fast path: already cached → bump recency and return.
        if let Some(ctx) = self.cached(project_id) {
            ctx.touch();
            return Ok(ctx);
        }

        // Build (opens DB + hydrates) while holding no locks.
        let ctx = ProjectContext::build(path).await?;
        ctx.touch();

        // Reserve capacity + record recency under a brief synchronous lock.
        let evicted = {
            let mut lru = self.lru.lock();
            let evicted =
                if !lru.contains(&project_id) && lru.len() >= self.config.max_active_projects {
                    lru.pop_lru().map(|(id, ())| id)
                } else {
                    None
                };
            lru.put(project_id, ());
            evicted
        };
        if let Some(evicted_id) = evicted {
            // Drop the cached Arc → memory freed once all holders release.
            // The DB file and registry row are untouched.
            self.projects.remove(&evicted_id);
        }
        self.projects.insert(project_id, Arc::clone(&ctx));
        Ok(ctx)
    }

    /// Get a project by path, lazily re-hydrating it if it was evicted.
    pub async fn get(&self, path: &Path) -> Result<Arc<ProjectContext>, CoreError> {
        self.attach(path).await
    }

    /// Detach a project: drop its in-memory state and mark the registry row
    /// `archived`. The DB file is preserved.
    pub async fn detach(&self, project_id: ProjectId) -> Result<(), CoreError> {
        self.projects.remove(&project_id);
        self.lru.lock().pop(&project_id);
        tokio::task::spawn_blocking(move || -> Result<(), CoreError> {
            let registry = open_registry()?;
            archive_project(&registry, project_id)
        })
        .await
        .expect("detach task panicked")
    }

    /// Whether a project is currently held in memory.
    pub fn is_active(&self, project_id: ProjectId) -> bool {
        self.projects.contains_key(&project_id)
    }

    /// Number of currently-active projects.
    pub fn active_count(&self) -> usize {
        self.projects.len()
    }

    /// Summaries of all active projects.
    pub fn list_active(&self) -> Vec<ProjectSummary> {
        self.projects
            .iter()
            .map(|entry| {
                let ctx = entry.value();
                ProjectSummary {
                    project_id: ctx.project_id(),
                    canonical_path: ctx.canonical_path().to_path_buf(),
                    symbol_count: ctx.graph().symbol_count(),
                    last_accessed: ctx.last_accessed(),
                }
            })
            .collect()
    }

    /// Look up a cached context (cloning the `Arc` out before releasing the map
    /// guard) and bump its LRU recency. Returns `None` if not cached.
    fn cached(&self, project_id: ProjectId) -> Option<Arc<ProjectContext>> {
        let ctx = self
            .projects
            .get(&project_id)
            .map(|entry| Arc::clone(entry.value()))?;
        self.lru.lock().promote(&project_id);
        Some(ctx)
    }
}

impl std::fmt::Debug for TenantManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantManager")
            .field("active", &self.projects.len())
            .field("max_active_projects", &self.config.max_active_projects)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mnemo_store::paths::project_db;

    fn temp_project() -> tempfile::TempDir {
        // A real, canonicalizable directory (resolve_project_id canonicalizes).
        tempfile::tempdir().unwrap()
    }

    #[tokio::test]
    async fn lru_eviction_releases_memory_keeps_db() {
        let mgr = TenantManager::new(TenantConfig {
            max_active_projects: 2,
        });
        let a = temp_project();
        let b = temp_project();
        let c = temp_project();

        let ctx_a = mgr.attach(a.path()).await.unwrap();
        let id_a = ctx_a.project_id();
        let ctx_b = mgr.attach(b.path()).await.unwrap();
        let id_b = ctx_b.project_id();
        let _ctx_c = mgr.attach(c.path()).await.unwrap();

        // `a` was least-recently-used → evicted; `b` and `c` remain.
        assert!(!mgr.is_active(id_a), "a should be evicted");
        assert!(mgr.is_active(id_b), "b should still be active");
        assert_eq!(mgr.active_count(), 2);

        // Eviction frees memory but keeps the DB on disk.
        assert!(
            project_db(id_a).exists(),
            "evicted project's DB must remain on disk"
        );

        // Re-attach `a` → lazy re-hydrate; now `b` (LRU) is evicted.
        let ctx_a2 = mgr.attach(a.path()).await.unwrap();
        assert_eq!(ctx_a2.project_id(), id_a);
        assert!(mgr.is_active(id_a));
        assert!(!mgr.is_active(id_b), "b should now be the evicted one");
    }

    #[tokio::test]
    async fn attach_is_idempotent() {
        let mgr = TenantManager::new(TenantConfig::default());
        let p = temp_project();
        let c1 = mgr.attach(p.path()).await.unwrap();
        let c2 = mgr.attach(p.path()).await.unwrap();
        assert_eq!(c1.project_id(), c2.project_id());
        assert_eq!(mgr.active_count(), 1);
        assert_eq!(mgr.list_active().len(), 1);
    }

    #[tokio::test]
    async fn detach_keeps_db_and_deactivates() {
        let mgr = TenantManager::new(TenantConfig::default());
        let p = temp_project();
        let id = mgr.attach(p.path()).await.unwrap().project_id();
        assert!(mgr.is_active(id));

        mgr.detach(id).await.unwrap();
        assert!(!mgr.is_active(id));
        assert!(project_db(id).exists(), "detach must keep the DB");
    }
}
