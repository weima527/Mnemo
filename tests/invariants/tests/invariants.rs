//! Project-wide invariants (PLAN M0.5 / DESIGN §10.1).
//!
//! These four tests are the safety net for the whole project. They must be
//! green from the first day and must never regress — in particular
//! `zero_pollution_basic`, which guards the contract that Mnemo never writes
//! into the user's repository.

use std::path::{Path, PathBuf};

/// Absolute path to `tests/fixtures/`.
fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
}

/// Recursively copy a directory tree.
fn copy_dir(src: &Path, dest: &Path) {
    std::fs::create_dir_all(dest).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dest.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Stage a named fixture into a fresh location.
fn stage_fixture(name: &str, dest: &Path) {
    copy_dir(&fixtures_root().join(name), dest);
}

/// Snapshot every file's relative path + content (sorted) for equality checks.
fn snapshot_dir(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.unwrap();
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        out.push((rel, std::fs::read(entry.path()).unwrap()));
    }
    out.sort();
    out
}

#[test]
fn zero_pollution_basic() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("proj");
    stage_fixture("basic_rust", &repo);

    let before = snapshot_dir(&repo);
    mnemo_index::index_repo(&repo, false).expect("index should succeed");
    let after = snapshot_dir(&repo);

    assert_eq!(
        before, after,
        "indexing must not create, modify, or delete any file in the project dir"
    );
}

#[test]
fn symbol_id_stable_across_reindex() {
    use mnemo_core::{ProjectId, SymbolIdentityId, SymbolKind};
    let project = ProjectId::from_canonical_path(Path::new("/tmp/x"));
    let id1 = SymbolIdentityId::derive(project, "src/lib.rs", "foo::bar", SymbolKind::Function);
    let id2 = SymbolIdentityId::derive(project, "src/lib.rs", "foo::bar", SymbolKind::Function);
    assert_eq!(id1, id2);
}

#[test]
fn project_id_resolves_through_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("real");
    std::fs::create_dir(&target).unwrap();
    let link = tmp.path().join("link");

    // Symlink creation needs privilege on Windows; skip gracefully if denied.
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&target, &link).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(&target, &link).is_ok();

    if !made {
        eprintln!("skipping symlink invariant: cannot create symlinks here");
        return;
    }

    let (id_target, _) = mnemo_store::paths::resolve_project_id(&target).unwrap();
    let (id_link, _) = mnemo_store::paths::resolve_project_id(&link).unwrap();
    assert_eq!(id_target, id_link, "symlink must resolve to the canonical id");
}

#[cfg(unix)]
#[test]
fn readonly_project_indexable() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("proj");
    stage_fixture("basic_rust", &repo);

    // Make the whole project read-only, then prove indexing still works
    // (the DB lives under ~/.mnemo, not the repo).
    set_mode_recursive(&repo, 0o555);
    let result = mnemo_index::index_repo(&repo, false);
    // Restore write perms so tempfile cleanup can remove the dir.
    set_mode_recursive(&repo, 0o755);

    assert!(
        result.is_ok(),
        "a read-only project must still index: {:?}",
        result.err()
    );
}

#[cfg(unix)]
fn set_mode_recursive(root: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.unwrap();
        let _ = std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(mode));
    }
}
