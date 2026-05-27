//! Repository indexing engine.
//!
//! Orchestrates the pipeline:
//! 1. Resolve the repo to a stable [`ProjectId`] (canonical path).
//! 2. Open the per-project index DB under `~/.mnemo/projects/<id>/` — the
//!    target repository is **never** written to (zero-pollution contract).
//! 3. Walk the repo, deriving a stable [`FileIdentityId`] per supported file.
//! 4. Parse each file → raw symbols + raw edges; assign content-derived
//!    identity/version ids.
//! 5. Resolve raw edges against a project-wide symbol index.
//! 6. Persist files, symbols, and edges to the store in a single transaction,
//!    skipping rows whose content is unchanged (incremental MVCC).
//!
//! This is the primary entrypoint for `index_repo`.

pub mod context;
pub mod overlay;
pub mod query;

use mnemo_core::{
    CoreError, FileIdentityId, Language, ProjectId, Range, RawEdge, SnapshotId, SymbolIdentityId,
    SymbolKind, SymbolVersionId,
};
use mnemo_parser::parse_file;
use mnemo_resolve::{resolve_file_edges, ResolvedSymbol, SymbolIndex};
use mnemo_store::dao::{
    edge as edge_dao, file as file_dao, meta as meta_dao, snapshot as snapshot_dao,
    symbol as symbol_dao,
};
use mnemo_store::open_database;
use mnemo_store::paths::{ensure_home_layout, project_db, resolve_project_id};
use mnemo_store::registry::{open_registry, update_index_stats, upsert_project};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Confidence stored for parser-derived edges (0–100). Definite syntactic
/// references are recorded at full confidence for the MVP.
const EDGE_CONFIDENCE: i64 = 100;

/// Result of a full index operation.
#[derive(Debug, Clone)]
pub struct IndexResult {
    /// Stable identity of the indexed project.
    pub project_id: ProjectId,
    /// Absolute path to the per-project index database (under `~/.mnemo/`).
    pub db_path: PathBuf,
    /// The snapshot created by this index run.
    pub snapshot: SnapshotId,
    /// Number of supported files walked.
    pub file_count: usize,
    /// Symbol *versions* newly persisted this run (0 when nothing changed).
    pub symbol_count: usize,
    /// Edges newly persisted this run (0 when nothing changed).
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

/// A walked source file: stable identity, repo-relative path, language, source.
struct WalkedFile {
    file_id: FileIdentityId,
    rel_path: String,
    abs_path: PathBuf,
    language: Language,
    contents: String,
}

/// A parser symbol finalized with its content-derived identity/version ids.
struct FinalSymbol {
    identity: SymbolIdentityId,
    version: SymbolVersionId,
    file_id: FileIdentityId,
    qualified_name: String,
    name: String,
    kind: SymbolKind,
    range: Range,
    /// blake3 of the symbol's source, hex-encoded (stored in `content_hash`).
    content_hash: String,
}

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
    let mut conn = open_database(&db_path)?;

    if force {
        tracing::info!("force re-index requested; MVP re-walks all files (history preserved)");
    }

    // 5. Walk + parse every supported file, finalizing symbol identities and
    //    building the project-wide resolution index.
    let files = walk_repo(repo_path, project_id)?;
    let total_files = files.len();

    let mut resolved_symbols: Vec<ResolvedSymbol> = Vec::new();
    let mut per_file: Vec<(WalkedFile, Vec<FinalSymbol>, Vec<RawEdge>)> = Vec::new();
    let mut seen_identities: HashSet<SymbolIdentityId> = HashSet::new();

    for file in files {
        let parsed = parse_file(file.file_id, &file.abs_path, &file.contents);
        let mut finals = Vec::with_capacity(parsed.symbols.len());
        for raw in parsed.symbols {
            let identity =
                SymbolIdentityId::derive(project_id, &file.rel_path, &raw.qualified_name, raw.kind);
            // A file may yield two symbols with the same (qualified_name, kind);
            // they collapse to one logical identity — keep the first.
            if !seen_identities.insert(identity) {
                continue;
            }
            let version = SymbolVersionId::derive(identity, &raw.content_hash);
            resolved_symbols.push(ResolvedSymbol {
                identity,
                file: file.file_id,
                name: raw.name.clone(),
                qualified_name: raw.qualified_name.clone(),
            });
            finals.push(FinalSymbol {
                identity,
                version,
                file_id: file.file_id,
                qualified_name: raw.qualified_name,
                name: raw.name,
                kind: raw.kind,
                range: raw.definition_range,
                content_hash: raw.content_hash.to_hex().to_string(),
            });
        }
        per_file.push((file, finals, parsed.edges));
    }

    let index = SymbolIndex::build(resolved_symbols);

    // Resolve raw edges per file against the project-wide index.
    let mut resolved_edges = Vec::new();
    for (file, _finals, raw_edges) in &per_file {
        let (edges, _unresolved) = resolve_file_edges(&index, file.file_id, raw_edges);
        resolved_edges.extend(edges);
    }

    // 6. Persist everything in a single transaction. Unchanged symbols/edges
    //    produce no new rows (incremental).
    let mut new_symbols = 0usize;
    let mut new_edges = 0usize;
    let snapshot;
    {
        let tx = conn.transaction()?;
        meta_dao::set(&tx, "project_uuid", &project_id.to_hex())?;
        let parent = snapshot_dao::latest(&tx)?;
        snapshot = snapshot_dao::create(&tx, "filehash", None, None, parent)?;

        for (file, finals, _edges) in &per_file {
            file_dao::upsert_identity(
                &tx,
                file.file_id,
                &file.rel_path,
                language_name(file.language),
            )?;
            for sym in finals {
                symbol_dao::upsert_identity(
                    &tx,
                    sym.identity,
                    sym.file_id,
                    &sym.qualified_name,
                    &sym.name,
                    sym.kind,
                )?;
                // Unchanged content → keep the existing open version.
                if !symbol_dao::open_version_matches(&tx, sym.identity, &sym.content_hash)? {
                    symbol_dao::close_open_versions(&tx, sym.identity, snapshot)?;
                    symbol_dao::insert_version(
                        &tx,
                        sym.version,
                        sym.identity,
                        &sym.range,
                        &sym.content_hash,
                        snapshot,
                    )?;
                    new_symbols += 1;
                }
            }
        }

        for edge in &resolved_edges {
            if !edge_dao::open_exists(&tx, edge.from, edge.to, edge.kind)? {
                edge_dao::insert_version(
                    &tx,
                    edge.from,
                    edge.to,
                    edge.kind,
                    EDGE_CONFIDENCE,
                    snapshot,
                )?;
                new_edges += 1;
            }
        }

        tx.commit()?;
    }

    // Record index stats in the registry.
    let db_size = std::fs::metadata(&db_path)
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    update_index_stats(&registry, project_id, db_size)?;

    tracing::info!(
        snapshot = %snapshot,
        files = total_files,
        new_symbols,
        new_edges,
        "index complete"
    );

    Ok(IndexResult {
        project_id,
        db_path,
        snapshot,
        file_count: total_files,
        symbol_count: new_symbols,
        edge_count: new_edges,
        elapsed_ms: started.elapsed().as_millis() as u64,
        index_version: 1,
    })
}

/// Map a [`Language`] to the string stored in `file_identity.language`.
fn language_name(language: Language) -> &'static str {
    match language {
        Language::Rust => "rust",
        Language::TypeScript => "typescript",
    }
}

/// Walk the repository, returning a [`WalkedFile`] for every supported file.
/// The identity is derived from the project and the repo-relative,
/// forward-slash-normalized path (stable across re-index).
fn walk_repo(root: &Path, project_id: ProjectId) -> Result<Vec<WalkedFile>, CoreError> {
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
        let Some(language) = Language::from_extension(ext) else {
            continue;
        };

        let contents = std::fs::read_to_string(path)?;
        let rel_path = repo_relative_path(root, path);
        let file_id = FileIdentityId::derive(project_id, &rel_path);

        results.push(WalkedFile {
            file_id,
            rel_path,
            abs_path: path.to_path_buf(),
            language,
            contents,
        });
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
        assert!(is_ignored(
            Path::new("/repo/node_modules/pkg/index.js"),
            root
        ));
        assert!(!is_ignored(Path::new("/repo/src/main.rs"), root));
    }

    #[test]
    fn repo_relative_path_uses_forward_slashes() {
        let root = Path::new("/repo");
        let p = Path::new("/repo/src/auth/login.rs");
        assert_eq!(repo_relative_path(root, p), "src/auth/login.rs");
    }
}
