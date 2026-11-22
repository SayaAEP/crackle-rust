use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

fn fixture(contents: &str) -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = env::temp_dir().join(format!("crackle-config-{id}.nix"));
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn reads_and_updates_home_packages_without_touching_other_blocks() {
    let path = fixture(
        "{ pkgs, ... }: {\n  home.packages = with pkgs; [\n    git\n  ];\n  programs.bash.enable = true;\n}\n",
    );
    assert_eq!(read_packages(&path, "home").unwrap(), ["git"]);
    update_packages(&path, "home", &["nixpkgs.neovim".into()], &[], false).unwrap();
    let updated = fs::read_to_string(&path).unwrap();
    assert!(updated.contains("    pkgs.neovim\n    git\n"));
    assert!(updated.contains("programs.bash.enable"));
    fs::remove_file(path).unwrap();
}

#[test]
fn ignores_duplicate_blocks_without_touching_the_file_and_removes_only_requested_packages() {
    let duplicate =
        fixture("{\n  home.packages = [\n    git\n  ];\n  home.packages = [\n    vim\n  ];\n}\n");
    let original = fs::read_to_string(&duplicate).unwrap();
    assert!(read_packages(&duplicate, "home").unwrap().is_empty());
    assert!(
        update_packages(&duplicate, "home", &["neovim".into()], &[], false)
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::read_to_string(&duplicate).unwrap(), original);
    fs::remove_file(duplicate).unwrap();

    let path = fixture(
        "{\n  home.packages = [\n    git\n    vim\n  ];\n  programs.bash.enable = true;\n}\n",
    );
    update_packages(&path, "home", &[], &["git".into()], false).unwrap();
    let updated = fs::read_to_string(&path).unwrap();
    assert!(!updated.contains("    git\n"));
    assert!(updated.contains("    vim\n"));
    assert!(updated.contains("programs.bash.enable"));
    fs::remove_file(path).unwrap();
}

#[test]
fn reads_system_packages_using_the_legacy_block_name() {
    let path =
        fixture("{ ... }: {\n  environment.systemPackages = with pkgs; [\n    git\n  ];\n}\n");
    assert_eq!(read_packages(&path, "system").unwrap(), ["git"]);
    assert!(read_packages(&path, "home").unwrap().is_empty());
    fs::remove_file(path).unwrap();
}

#[test]
fn preserves_crlf_line_endings_while_rewriting_the_block() {
    let path = fixture(
        "{\r\n  home.packages = [\r\n    git\r\n  ];\r\n  programs.bash.enable = true;\r\n}\r\n",
    );
    update_packages(&path, "home", &["vim".into()], &[], false).unwrap();
    let updated = fs::read_to_string(&path).unwrap();
    assert_eq!(
        updated,
        "{\r\n  home.packages = [\r\n    vim\r\n    git\r\n  ];\r\n  programs.bash.enable = true;\r\n}\r\n"
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn only_rewrites_the_exact_nixpkgs_prefix() {
    let path = fixture("{\n  home.packages = [\n    git\n  ];\n}\n");
    update_packages(
        &path,
        "home",
        &["nixpkgs.neovim".into(), "nixpkgsomething".into()],
        &[],
        false,
    )
    .unwrap();
    let updated = fs::read_to_string(&path).unwrap();
    assert!(updated.contains("    pkgs.neovim\n"));
    assert!(updated.contains("    nixpkgsomething\n"));
    fs::remove_file(path).unwrap();
}

#[test]
fn preserves_a_missing_final_newline() {
    let path = fixture("{\n  home.packages = [\n    git\n  ];\n}");
    update_packages(&path, "home", &["vim".into()], &[], false).unwrap();
    let updated = fs::read_to_string(&path).unwrap();
    assert!(updated.ends_with("};") || updated.ends_with("}"));
    assert!(updated.contains("    vim\n    git\n"));
    fs::remove_file(path).unwrap();
}
