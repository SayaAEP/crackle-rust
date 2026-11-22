use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

fn test_root(label: &str) -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    env::temp_dir().join(format!("crackle-sudo-{label}-{id}"))
}

#[test]
fn existing_untracked_command_is_not_a_crackle_owned_link() {
    let root = test_root("untracked");
    let snap_bin = root.join("snap/bin");
    let default_bin = root.join("root/.local/bin");
    fs::create_dir_all(&snap_bin).unwrap();
    fs::create_dir_all(&default_bin).unwrap();
    let existing = snap_bin.join("file");
    fs::write(&existing, b"existing Snap command").unwrap();
    let allowed_dirs = [default_bin, snap_bin];

    assert!(!sudo_link_matches_record(
        None,
        "file",
        &existing,
        Path::new("/snap/current/usr/bin/file"),
        &allowed_dirs,
    ));
    assert_eq!(fs::read(&existing).unwrap(), b"existing Snap command");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sudo_link_record_must_match_name_destination_and_target() {
    let root = test_root("record");
    let snap_bin = root.join("snap/bin");
    let default_bin = root.join("root/.local/bin");
    fs::create_dir_all(&snap_bin).unwrap();
    fs::create_dir_all(&default_bin).unwrap();
    let allowed_dirs = [default_bin, snap_bin.clone()];
    let link = snap_bin.join("file");
    let target = root.join("snap/current/usr/bin/file");
    let contents = format!("{}\n{}\n", link.display(), target.display());

    let record = parse_sudo_link_record("file", &contents, &allowed_dirs).unwrap();
    assert!(sudo_link_matches_record(
        Some(&record),
        "file",
        &link,
        &target,
        &allowed_dirs,
    ));
    assert!(!sudo_link_matches_record(
        Some(&record),
        "file",
        &link,
        Path::new("/snap/other/usr/bin/file"),
        &allowed_dirs,
    ));
    assert!(parse_sudo_link_record("other", &contents, &allowed_dirs).is_err());
    assert!(
        parse_sudo_link_record(
            "file",
            &format!(
                "{}\n{}\n",
                root.join("outside/file").display(),
                target.display()
            ),
            &allowed_dirs,
        )
        .is_err()
    );

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn sudo_link_record_rejects_a_retargeted_symlink() {
    use std::os::unix::fs::symlink;

    let root = test_root("retargeted");
    let snap_bin = root.join("snap/bin");
    let default_bin = root.join("root/.local/bin");
    fs::create_dir_all(&snap_bin).unwrap();
    fs::create_dir_all(&default_bin).unwrap();
    let allowed_dirs = [default_bin, snap_bin.clone()];
    let link = snap_bin.join("file");
    let target = root.join("snap/current/usr/bin/file");
    let replacement = root.join("snap/other/usr/bin/file");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::create_dir_all(replacement.parent().unwrap()).unwrap();
    fs::write(&target, b"original").unwrap();
    fs::write(&replacement, b"replacement").unwrap();
    symlink(&target, &link).unwrap();
    let contents = format!("{}\n{}\n", link.display(), target.display());
    let record = parse_sudo_link_record("file", &contents, &allowed_dirs).unwrap();

    assert!(sudo_link_matches_record(
        Some(&record),
        "file",
        &link,
        &fs::read_link(&link).unwrap(),
        &allowed_dirs,
    ));
    fs::remove_file(&link).unwrap();
    symlink(&replacement, &link).unwrap();
    assert!(!sudo_link_matches_record(
        Some(&record),
        "file",
        &link,
        &fs::read_link(&link).unwrap(),
        &allowed_dirs,
    ));

    fs::remove_dir_all(root).unwrap();
}
