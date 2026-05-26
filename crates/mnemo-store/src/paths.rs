//! Filesystem path resolution for Mnemo.
//!
//! Per DESIGN §3: all Mnemo data lives under `~/.mnemo/`.
//! The target project directory is never written to (zero-pollution).
//!
//! Platform notes:
//! - Linux/macOS: `~` = `$HOME`
//! - Windows: `~` = `%USERPROFILE%` (e.g. `C:\Users\marco\.mnemo\`)

use mnemo_core::{CoreError, ProjectId};
use std::path::{Path, PathBuf};

/// Returns the user's Mnemo home directory (`~/.mnemo/`).
pub fn home_mnemo() -> PathBuf {
    let base = dirs::home_dir().expect("cannot determine home directory");
    base.join(".mnemo")
}

/// Returns the path to the global registry database.
pub fn registry_db_path() -> PathBuf {
    home_mnemo().join("registry.db")
}

/// Returns the path to the daemon socket.
///
/// Unix: `~/.mnemo/daemon.sock`
/// Windows: `\\.\pipe\mnemo-daemon` (UNIX sockets not available)
pub fn daemon_socket_path() -> PathBuf {
    #[cfg(unix)]
    {
        home_mnemo().join("daemon.sock")
    }
    #[cfg(windows)]
    {
        PathBuf::from(r"\\.\pipe\mnemo-daemon")
    }
}

/// Returns the per-project directory root.
///
/// The directory name is the **full** 32-char hex id (`to_hex`), not the
/// truncated `Display` form, so distinct projects never collide.
///
/// Example: `~/.mnemo/projects/a3f5e1c9.../`
pub fn project_dir(id: ProjectId) -> PathBuf {
    home_mnemo().join("projects").join(id.to_hex())
}

/// Returns the path to a project's main index database.
///
/// Example: `~/.mnemo/projects/a3f5e1c9.../index.db`
pub fn project_db(id: ProjectId) -> PathBuf {
    project_dir(id).join("index.db")
}

/// Returns config file path.
pub fn config_path() -> PathBuf {
    home_mnemo().join("config.toml")
}

/// Returns daemon log path.
pub fn daemon_log_path() -> PathBuf {
    home_mnemo().join("daemon.log")
}

/// Ensure the `~/.mnemo/` directory layout exists.
///
/// Creates `~/.mnemo/projects/` if it doesn't exist.
/// Idempotent — safe to call repeatedly.
pub fn ensure_home_layout() -> Result<(), CoreError> {
    let home = home_mnemo();
    std::fs::create_dir_all(&home).map_err(CoreError::Io)?;
    std::fs::create_dir_all(home.join("projects")).map_err(CoreError::Io)?;
    Ok(())
}

/// Resolve a project path to its canonical absolute path and stable `ProjectId`.
///
/// This is the main entrypoint for converting a user-supplied path (which
/// may be relative, contain symlinks, or have inconsistent separators) into
/// a stable project identifier.
///
/// Returns `(ProjectId, canonical_path)`.
///
/// # Platform normalization
/// - Symlinks and `..` resolved via `std::fs::canonicalize`.
/// - Windows drive letters lowercased.
///
/// The `ProjectId` is derived from the canonical absolute path as-is (native
/// separators). It is intentionally machine-/platform-specific — the same repo
/// on a different machine is a different project.
pub fn resolve_project_id(input: &Path) -> Result<(ProjectId, PathBuf), CoreError> {
    // Canonicalize: resolves `..`, symlinks, normalizes separators.
    let canonical = std::fs::canonicalize(input).map_err(|e| {
        CoreError::NotFound("path", format!("{:?}: {e}", input))
    })?;

    // Platform normalization: Windows drive letter to lowercase.
    #[cfg(windows)]
    let canonical = {
        let s = canonical.to_string_lossy().to_string();
        if s.len() > 1 && s.as_bytes()[1] == b':' {
            let mut chars: Vec<char> = s.chars().collect();
            chars[0] = chars[0].to_ascii_lowercase();
            PathBuf::from(chars.into_iter().collect::<String>())
        } else {
            canonical
        }
    };

    let id = ProjectId::from_canonical_path(&canonical);
    Ok((id, canonical))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_mnemo_is_not_empty() {
        let h = home_mnemo();
        assert!(h.to_string_lossy().contains(".mnemo"));
    }

    #[test]
    fn ensure_home_layout_succeeds() {
        ensure_home_layout().unwrap();
        assert!(home_mnemo().exists());
        assert!(home_mnemo().join("projects").exists());
    }

    #[test]
    fn resolve_project_id_consistent() {
        // Same canonical path → same id
        let (id1, _) = resolve_project_id(Path::new(".")).unwrap();
        let cwd = std::env::current_dir().unwrap();
        let (id2, _) = resolve_project_id(&cwd).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn project_db_is_under_home() {
        let id = ProjectId::ZERO;
        let db = project_db(id);
        let db_str = db.to_string_lossy();
        assert!(db_str.contains(".mnemo"));
        assert!(db_str.ends_with("index.db"));
    }

    #[test]
    fn resolve_project_id_with_dot_dot_normalizes() {
        // ./foo/../bar should resolve to the canonical bar path
        let cwd = std::env::current_dir().unwrap();
        let parent = cwd.parent().unwrap_or(Path::new("/"));
        let dot_dot = cwd.join("..");
        let (id1, _) = resolve_project_id(&dot_dot).unwrap();
        let (id2, _) = resolve_project_id(parent).unwrap();
        assert_eq!(id1, id2);
    }

    // ---- DESIGN §3.4 exit-criteria tests ----

    #[test]
    fn same_project_different_paths_resolve_same_id() {
        // A directory reached directly and via `<dir>/sub/..` must map to one id.
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let indirect = sub.join(".."); // == dir.path() after canonicalize

        let (id_direct, _) = resolve_project_id(dir.path()).unwrap();
        let (id_indirect, _) = resolve_project_id(&indirect).unwrap();
        assert_eq!(id_direct, id_indirect);
    }

    #[test]
    fn symlinked_project_resolves_to_canonical() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real_project");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("link_project");

        // Creating symlinks on Windows needs Developer Mode / admin; skip if denied.
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&target, &link).is_ok();

        if !made {
            eprintln!("skipping symlink test: cannot create symlinks here");
            return;
        }

        let (id_target, _) = resolve_project_id(&target).unwrap();
        let (id_link, _) = resolve_project_id(&link).unwrap();
        assert_eq!(id_target, id_link, "symlink must resolve to the canonical id");
    }

    #[test]
    fn moved_project_gets_new_id_until_path_rebind() {
        // Two distinct locations are two distinct projects. Re-binding a moved
        // project to its old id is a future explicit `project rebind` operation.
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (id_a, _) = resolve_project_id(a.path()).unwrap();
        let (id_b, _) = resolve_project_id(b.path()).unwrap();
        assert_ne!(id_a, id_b);
    }
}
