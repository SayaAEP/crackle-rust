use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root() -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("crackle-cleanup-{id}"));
    fs::create_dir_all(&root).unwrap();
    root
}

fn workspace_for(root: &Path) -> Cleanup {
    let mut cleanup = workspace(
        root.join("prefix"),
        root.join("data/crackle"),
        root.join("cache/apt"),
        root.join("state/apt"),
        root.join("config/apt"),
        root.join("state/dpkg"),
        root.join("state"),
        root.join("prefix/lib/crackle"),
        crate::COMMAND_NAMES,
    );
    cleanup.sudo_clean = false;
    cleanup
}

#[test]
fn removes_only_marker_owned_apt_dirs_and_keeps_unrelated_broken_links() {
    let root = temp_root();
    let cleanup = workspace_for(&root);
    fs::create_dir_all(cleanup.apt_cache.join("archives")).unwrap();
    fs::create_dir_all(cleanup.apt_state.join("lists")).unwrap();
    mark_owned(&cleanup.apt_cache);
    mark_owned(&cleanup.apt_state);

    let unrelated = cleanup.prefix.join("bin/user-tool");
    fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/nonexistent/user-target", &unrelated).unwrap();
    #[cfg(not(unix))]
    fs::write(&unrelated, b"x").unwrap();

    cleanup.run();

    assert!(
        !cleanup.apt_cache.exists(),
        "marker-owned cache was removed"
    );
    assert!(
        !cleanup.apt_state.exists(),
        "marker-owned state was removed"
    );
    assert!(
        !cleanup.apt_config.exists(),
        "a directory Crackle never created is left alone"
    );
    assert!(
        fs::symlink_metadata(&unrelated).is_ok(),
        "unrelated link is preserved"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn removes_crackle_package_area_even_without_marker() {
    let root = temp_root();
    let cleanup = workspace_for(&root);
    fs::create_dir_all(cleanup.pkg_path.join("packages/xz-utils/usr/bin")).unwrap();
    fs::create_dir_all(cleanup.pkg_path.join("crackle-apt-123")).unwrap();
    cleanup.run();
    assert!(!cleanup.pkg_path.join("packages").exists());
    assert!(!cleanup.pkg_path.join("crackle-apt-123").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn side_by_side_cleanup_keeps_legacy_library_and_completion() {
    let root = temp_root();
    let mut cleanup = workspace_for(&root);
    cleanup.library_dir = cleanup.prefix.join("lib/crackle-rust");

    let legacy_library = cleanup.prefix.join("lib/crackle/basefunc");
    let legacy_completion = root.join("data/bash-completion/completions/crackle");
    let side_by_side_library = cleanup.library_dir.join("microsoft");
    fs::create_dir_all(legacy_library.parent().unwrap()).unwrap();
    fs::create_dir_all(legacy_completion.parent().unwrap()).unwrap();
    fs::create_dir_all(side_by_side_library.parent().unwrap()).unwrap();
    fs::write(&legacy_library, "legacy").unwrap();
    fs::write(&legacy_completion, "legacy").unwrap();
    fs::write(&side_by_side_library, "rust").unwrap();

    cleanup.run();

    assert!(legacy_library.is_file(), "legacy library is preserved");
    assert!(
        legacy_completion.is_file(),
        "legacy completion is preserved"
    );
    assert!(!cleanup.library_dir.exists(), "own library is removed");
    fs::remove_dir_all(root).unwrap();
}
