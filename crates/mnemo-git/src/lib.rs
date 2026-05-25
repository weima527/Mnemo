//! Git integration: diffs, refs, commits, and working-tree overlay detection.
//!
//! This crate wraps `git2` to:
//! - Detect changed files between two refs or between HEAD and working tree.
//! - Resolve commit SHAs and ref names.
//! - Produce structured diffs for overlay construction.

use mnemo_core::CoreError;
use std::path::Path;

/// A structured diff between two git trees.
#[derive(Debug, Clone)]
pub struct GitDiff {
    /// Files that were added.
    pub added: Vec<DiffEntry>,
    /// Files that were modified.
    pub modified: Vec<DiffEntry>,
    /// Files that were deleted.
    pub deleted: Vec<DiffEntry>,
}

/// A single changed file in a diff.
#[derive(Debug, Clone)]
pub struct DiffEntry {
    /// Repository-relative path of the file.
    pub path: String,
    /// Kind of change.
    pub change_kind: ChangeKind,
}

/// Kind of file change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// New file.
    Added,
    /// Existing file modified.
    Modified,
    /// File deleted.
    Deleted,
}

/// Detect changed files between two git refs.
///
/// `base` and `head` can be branch names, tag names, or commit SHAs.
/// For the working tree, use `detect_working_tree_changes`.
pub fn diff_refs(
    repo_path: &Path,
    base: &str,
    head: &str,
) -> Result<GitDiff, CoreError> {
    let repo = git2::Repository::open(repo_path)?;

    let base_tree = resolve_tree(&repo, base)?;
    let head_tree = resolve_tree(&repo, head)?;

    let diff = repo.diff_tree_to_tree(
        Some(&base_tree),
        Some(&head_tree),
        None,
    )?;

    collect_diff_entries(&diff)
}

/// Detect changed files in the working tree (unstaged + staged).
pub fn detect_working_tree_changes(repo_path: &Path) -> Result<GitDiff, CoreError> {
    let repo = git2::Repository::open(repo_path)?;

    // Diff HEAD against working tree (includes staged changes).
    let head_tree = match repo.head() {
        Ok(head) => Some(head.peel_to_tree()?),
        Err(_) => None, // No commits yet — treat as all-added.
    };

    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(false);

    let diff = repo.diff_tree_to_workdir_with_index(
        head_tree.as_ref(),
        Some(&mut opts),
    )?;

    collect_diff_entries(&diff)
}

/// Get the commit SHA for a given ref (branch, tag, or "HEAD").
pub fn resolve_commit_sha(repo_path: &Path, ref_name: &str) -> Result<String, CoreError> {
    let repo = git2::Repository::open(repo_path)?;
    let obj = repo.revparse_single(ref_name)?;
    Ok(obj.id().to_string())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn resolve_tree<'a>(
    repo: &'a git2::Repository,
    ref_name: &str,
) -> Result<git2::Tree<'a>, CoreError> {
    let obj = repo.revparse_single(ref_name)?;
    Ok(obj.peel_to_tree()?)
}

fn collect_diff_entries(diff: &git2::Diff) -> Result<GitDiff, CoreError> {
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();

    diff.foreach(
        &mut |delta, _| {
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .and_then(|p| p.to_str())
                .map(|s| s.to_string());

            if let Some(path) = path {
                let entry = DiffEntry {
                    path,
                    change_kind: match delta.status() {
                        git2::Delta::Added => ChangeKind::Added,
                        git2::Delta::Deleted => ChangeKind::Deleted,
                        _ => ChangeKind::Modified,
                    },
                };
                match entry.change_kind {
                    ChangeKind::Added => added.push(entry),
                    ChangeKind::Modified => modified.push(entry),
                    ChangeKind::Deleted => deleted.push(entry),
                }
            }
            true
        },
        None,
        None,
        None,
    )?;

    Ok(GitDiff {
        added,
        modified,
        deleted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_head_on_non_repo() {
        // Should fail gracefully — not a git repo.
        let result = resolve_commit_sha(Path::new("/tmp/nonexistent_abc"), "HEAD");
        assert!(result.is_err());
    }
}
