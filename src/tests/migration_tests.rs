use super::*;
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn rollback_restores_managed_files_and_removes_new_ones() {
    let root = env::temp_dir().join(format!(
        "crackle-migration-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let prefix = root.join("prefix");
    let data = root.join("data");
    let state = root.join("state");
    fs::create_dir_all(prefix.join("bin")).unwrap();
    fs::create_dir_all(data.join("bash-completion/completions")).unwrap();
    fs::write(prefix.join("bin/crackle"), "legacy").unwrap();
    fs::write(prefix.join("bin/user-tool"), "untouched").unwrap();
    let completion = data.join("bash-completion/completions/crackle");
    fs::write(&completion, "legacy completion").unwrap();
    let paths = vec![
        ManagedPath {
            root: Root::Prefix,
            relative: PathBuf::from("bin/crackle"),
        },
        ManagedPath {
            root: Root::Prefix,
            relative: PathBuf::from("bin/nix-manager"),
        },
        ManagedPath {
            root: Root::Data,
            relative: PathBuf::from("bash-completion/completions/crackle"),
        },
    ];
    let backup = create_backup(&prefix, &data, &state, &paths).unwrap();
    fs::write(prefix.join("bin/crackle"), "new").unwrap();
    fs::write(prefix.join("bin/nix-manager"), "new").unwrap();
    fs::write(&completion, "new completion").unwrap();
    restore_backup(&prefix, &data, &backup).unwrap();
    assert_eq!(
        fs::read_to_string(prefix.join("bin/crackle")).unwrap(),
        "legacy"
    );
    assert!(!prefix.join("bin/nix-manager").exists());
    assert_eq!(
        fs::read_to_string(prefix.join("bin/user-tool")).unwrap(),
        "untouched"
    );
    assert_eq!(
        fs::read_to_string(&completion).unwrap(),
        "legacy completion"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn manifest_and_relative_paths_handle_both_roots() {
    let entries =
        parse_manifest("present\tprefix\tbin/crackle\nabsent\tdata\tbash-completion/x\n").unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].root, Root::Prefix);
    assert_eq!(entries[1].root, Root::Data);
    let legacy = parse_manifest("present\tbin/crackle\n").unwrap();
    assert_eq!(legacy[0].root, Root::Prefix);
    assert!(parse_manifest("present\tdata\t../escape\n").is_err());
}
