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
/// Example: `~/.mnemo/projects/a3f5e1c9.../`
pub fn project_dir(id: ProjectId) -> PathBuf {
    home_mnemo().join("projects").join(id.to_string())
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
/// - Symlinks resolved via `std::fs::canonicalize`.
/// - Windows drive letters lowercased.
/// - Path separators normalized to forward slashes in the hash input.
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
}
