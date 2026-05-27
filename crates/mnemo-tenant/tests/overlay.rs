//! M2.4 exit test: working-tree overlay edits are visible immediately and never
//! write the DB.

use mnemo_index::index_repo;
use mnemo_index::overlay::OverlayFile;
use mnemo_tenant::{TenantConfig, TenantManager};
use std::path::{Path, PathBuf};

/// A modified `math.rs` that adds a `triple` function (calling `add`).
const MODIFIED_MATH: &str = "\
//! Modified in the working-tree overlay.
pub fn add(a: i64, b: i64) -> i64 { a + b }
pub fn mul(a: i64, b: i64) -> i64 { a * b }
pub fn square(n: i64) -> i64 { mul(n, n) }
pub fn triple(n: i64) -> i64 { add(n, add(n, n)) }
";

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
}

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

#[tokio::test]
async fn overlay_changes_visible_immediately_without_db_write() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("proj");
    copy_dir(&fixtures_root().join("basic_rust"), &repo);

    // Build the base index once (this is the only DB write).
    index_repo(&repo, false).unwrap();

    let mgr = TenantManager::new(TenantConfig::default());
    let ctx = mgr.attach(&repo).await.unwrap();

    // The base graph does not contain `triple`.
    assert!(ctx.graph().find_by_name("triple").is_empty());
    let db_size_before = std::fs::metadata(ctx.db_path()).unwrap().len();

    // Overlay a modified math.rs that adds `triple` — purely in memory.
    ctx.set_overlay(vec![OverlayFile {
        rel_path: "src/math.rs".to_string(),
        content: Some(MODIFIED_MATH.to_string()),
    }])
    .await
    .unwrap();

    // The new symbol is visible immediately, and base symbols remain.
    assert!(
        !ctx.graph().find_by_name("triple").is_empty(),
        "overlay symbol `triple` must be visible immediately"
    );
    assert!(!ctx.graph().find_by_name("square").is_empty());

    // The DB was not written.
    let db_size_after = std::fs::metadata(ctx.db_path()).unwrap().len();
    assert_eq!(
        db_size_before, db_size_after,
        "overlay must not write the DB"
    );

    // Clearing the overlay falls back to the base graph.
    ctx.clear_overlay();
    assert!(!ctx.has_overlay());
    assert!(ctx.graph().find_by_name("triple").is_empty());
    assert!(!ctx.graph().find_by_name("square").is_empty());
}
