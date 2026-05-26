//! Repository indexing engine.
//!
//! Orchestrates the pipeline:
//! 1. Resolve the repo to a stable [`ProjectId`] (canonical path).
//! 2. Open the per-project index DB under `~/.mnemo/projects/<id>/` — the
//!    target repository is **never** written to (zero-pollution contract).
//! 3. Walk the repo, deriving a stable [`FileIdentityId`] per supported file.
//! 4. Parse changed files → extract symbols / edges.
//! 5. (M1) Persist symbols and edges to the store in a single transaction.
//!
//! This is the primary entrypoint for `index_repo`.

use mnemo_core::{CoreError, FileIdentityId, Language, ProjectId};
use mnemo_parser::parse_file;
use mnemo_store::open_database;
use mnemo_store::paths::{ensure_home_layout, project_db, resolve_project_id};
use mnemo_store::registry::{open_registry, update_index_stats, upsert_project};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Result of a full index operation.
#[derive(Debug, Clone)]
pub struct IndexResult {
    /// Stable identity of the indexed project.
    pub project_id: ProjectId,
    /// Absolute path to the per-project index database (under `~/.mnemo/`).
    pub db_path: PathBuf,
    /// Number of files indexed.
    pub file_count: usize,
    /// Number of symbols extracted.
    pub symbol_count: usize,
    /// Number of edges constructed.
    pub edge_count: usize,
    /// Elapsed wall-clock time.
    pub elapsed_ms: u64,
    /// Index schema version at time of indexing.
    pub index_version: u32,
}

/// Default paths to ignore during indexing.
const DEFAULT_IGNORE_PATTERNS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".cache",
    ".venv",
    "venv",
    "__pycache__",
];

/// Index (or refresh) a repository at `repo_path`.
///
/// The index database lives in `~/.mnemo/projects/<project_id>/index.db`.
/// Nothing is ever written inside `repo_path` (DESIGN §3.2).
///
/// `force` currently re-walks every file; it does **not** wipe snapshot
/// history (a true reset is `project forget`, landing in M2).
pub fn index_repo(repo_path: &Path, force: bool) -> Result<IndexResult, CoreError> {
    let started = Instant::now();

    // 1. Resolve a stable project identity from the canonical path.
    let (project_id, canonical) = resolve_project_id(repo_path)?;

    // 2. Ensure the `~/.mnemo/` layout exists. The repo dir is never touched.
    ensure_home_layout()?;

    // 3. Register / refresh the project in the global registry.
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

    // 4. Open (create + migrate) the per-project DB under `~/.mnemo/`.
    let db_path = project_db(project_id);
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // The DB file is created and migrated as a side effect. Symbol/edge
    // persistence is wired in M1.3/M1.4; for M0 we only prove the DB lands
    // under `~/.mnemo/` (not the repo) and that the schema is in place.
    let _conn = open_database(&db_path)?;

    if force {
        tracing::info!("force re-index requested; M0 re-walks all files (history preserved)");
    }

    // 5. Walk supported files, deriving a stable identity for each.
    let files = walk_repo(repo_path, project_id)?;

    let mut total_symbols = 0usize;
    let total_files = files.len();
    for (file_id, path, contents) in &files {
        let result = parse_file(*file_id, path, contents);
        total_symbols += result.symbols.len();
        // TODO(M1.3/M1.4): persist symbols + edges via the DAO in one transaction.
    }

    // 6. Record index stats in the registry.
    let db_size = std::fs::metadata(&db_path)
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    update_index_stats(&registry, project_id, db_size)?;

    let elapsed = started.elapsed().as_millis() as u64;

    Ok(IndexResult {
        project_id,
        db_path,
        file_count: total_files,
        symbol_count: total_symbols,
        edge_count: 0, // TODO(M1.4): edge extraction + persistence
        elapsed_ms: elapsed,
        index_version: 1,
    })
}

/// Walk the repository, returning `(FileIdentityId, path, source)` for every
/// supported file. The identity is derived from the project and the
/// repo-relative, forward-slash-normalized path (stable across re-index).
fn walk_repo(
    root: &Path,
    project_id: ProjectId,
) -> Result<Vec<(FileIdentityId, PathBuf, String)>, CoreError> {
    let mut results = Vec::new();

    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_ignored(e.path(), root))
    {
        let entry = entry.map_err(|e| CoreError::Io(std::io::Error::other(e)))?;

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        if Language::from_extension(ext).is_none() {
            continue;
        }

        let contents = std::fs::read_to_string(path)?;
        let rel = repo_relative_path(root, path);
        let file_id = FileIdentityId::derive(project_id, &rel);

        results.push((file_id, path.to_path_buf(), contents));
    }

    Ok(results)
}

/// Compute the repo-relative path with forward-slash separators.
fn repo_relative_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// Check if a path should be excluded from indexing.
fn is_ignored(path: &Path, repo_root: &Path) -> bool {
    let rel = path.strip_prefix(repo_root).unwrap_or(path);
    let first_component = rel.components().next();

    if let Some(comp) = first_component {
        let name = comp.as_os_str().to_str().unwrap_or("");
        if DEFAULT_IGNORE_PATTERNS.contains(&name) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_ignored_detects_default_patterns() {
        let root = Path::new("/repo");
        assert!(is_ignored(Path::new("/repo/.git/config"), root));
        assert!(is_ignored(Path::new("/repo/node_modules/pkg/index.js"), root));
        assert!(!is_ignored(Path::new("/repo/src/main.rs"), root));
    }

    #[test]
    fn repo_relative_path_uses_forward_slashes() {
        let root = Path::new("/repo");
        let p = Path::new("/repo/src/auth/login.rs");
        assert_eq!(repo_relative_path(root, p), "src/auth/login.rs");
    }
}
