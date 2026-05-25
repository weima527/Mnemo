//! Repository indexing engine.
//!
//! Orchestrates the pipeline:
//! 1. Walk the repository file tree.
//! 2. Hash files, detect changes (incremental).
//! 3. Parse changed files → extract symbols.
//! 4. Insert / update symbols and edges in the store.
//! 5. Build or update the symbol graph.
//!
//! This is the primary entrypoint for `index_repo`.

use mnemo_core::{CoreError, FileIdentityId, Language};
use mnemo_git::detect_working_tree_changes;
use mnemo_parser::parse_file;
use mnemo_store::open_database;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Result of a full index operation.
#[derive(Debug, Clone)]
pub struct IndexResult {
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
/// Stores the index in `.mnemo/index.db` within the repo.
/// The `force` flag discards existing index data and starts fresh.
pub fn index_repo(repo_path: &Path, force: bool) -> Result<IndexResult, CoreError> {
    let started = Instant::now();

    let index_dir = repo_path.join(".mnemo");
    std::fs::create_dir_all(&index_dir)
        .map_err(|e| CoreError::Io(e))?;

    let db_path = index_dir.join("index.db");

    // Open or create the database.
    let conn = open_database(&db_path)?;

    // If force, drop and recreate schema.
    if force {
        conn.execute_batch("DROP TABLE IF EXISTS schema_version;")?;
        mnemo_store::schema::migrate(&conn)?;
    }

    // Walk files, filter to supported languages.
    let files = walk_repo(repo_path)?;

    let mut total_symbols = 0usize;
    let total_files = files.len();

    for (file_id, path, contents) in &files {
        let result = parse_file(*file_id, path, contents);
        total_symbols += result.symbols.len();
        // TODO: persist symbols and edges to SQLite.
        _ = &conn; // silence unused warning
    }

    let elapsed = started.elapsed().as_millis() as u64;

    Ok(IndexResult {
        file_count: total_files,
        symbol_count: total_symbols,
        edge_count: 0,     // TODO: edge extraction from parser
        elapsed_ms: elapsed,
        index_version: 1,
    })
}

/// Walk the repository, returning (FileIdentityId, path, source) for supported files.
fn walk_repo(root: &Path) -> Result<Vec<(FileIdentityId, PathBuf, String)>, CoreError> {
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

        let contents = std::fs::read_to_string(path).map_err(CoreError::Io)?;

        results.push((FileIdentityId::ZERO, path.to_path_buf(), contents)); // FIXME(M0.4): derive from project+path
    }

    Ok(results)
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
}
