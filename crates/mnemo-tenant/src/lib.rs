//! Multi-project tenancy for the Mnemo daemon (M2.1).
//!
//! [`TenantManager`] keeps several projects' hydrated
//! [`mnemo_graph::SymbolGraph`] caches in memory at once and evicts the
//! least-recently-used project when over [`TenantConfig::max_active_projects`].
//! Eviction frees memory only — it never deletes a project's DB or registry row
//! (DESIGN §4.4); [`TenantManager::detach`] additionally marks the registry row
//! archived.
//!
//! Queries are served from each [`ProjectContext`]'s lock-free `ArcSwap` graph
//! cache, so the DB is opened only for the rare operations that need it
//! (hydration, re-index), each on `tokio::task::spawn_blocking`. A connection
//! pool, memory/idle budgets, the working-tree overlay, and the git watcher are
//! deferred to later milestones (DESIGN §4.5 / M2.4 / M3).

mod context;
mod manager;

pub use context::{ProjectContext, Stats};
pub use manager::{ProjectSummary, TenantConfig, TenantManager};
