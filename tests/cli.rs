#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    stubs: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("crackle-cli-{name}-{id}"));
        let home = root.join("home");
        let stubs = root.join("stubs");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&stubs).unwrap();
        let log = root.join("calls.log");
        fs::write(&log, b"").unwrap();
        let fixture = Self {
            root,
            home,
            stubs,
            log,
        };
        fixture.prefix_stub("id", "echo 1000");
        fixture
    }

    fn prefix_stub(&self, name: &str, body: &str) {
        let directory = self.root.join("prefix/bin");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn stub(&self, name: &str, body: &str) {
        let path = self.stubs.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn recording_stub(&self, name: &str, exit: i32) {
        let log = self.log.display();
        self.stub(
            name,
            &format!("printf '%s %s\\n' \"{name}\" \"$*\" >> '{log}'\nexit {exit}"),
        );
    }

    fn binary_as(&self, name: &str) -> PathBuf {
        let destination = self.stubs.join(name);
        fs::copy(env!("CARGO_BIN_EXE_crackle"), &destination).unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
        destination
    }

    fn command(&self, program: &Path, args: &[&str]) -> Output {
        Command::new(program)
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", &self.stubs)
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("PKG_PATH", self.root.join("data/crackle"))
            .env("PKG_PREFIX", self.root.join("prefix"))
            .env("CRACKLE_SKIP_SUDO_CLEAN", "1")
            .output()
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(Path::new(env!("CARGO_BIN_EXE_crackle")), args)
    }

    fn calls(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn write_config(&self, contents: &str) -> PathBuf {
        let path = self.home.join(".config/home-manager/home.nix");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn root_guard_applies_to_crackle_but_not_helpers() {
    let fixture = Fixture::new("root");
    fixture.prefix_stub("id", "echo 0");

    let output = fixture.run(&["help"]);
    assert_eq!(output.status.code(), Some(100));
    assert!(stdout(&output).contains("Crackle: please don't run me as root"));

    let helper = fixture.binary_as("_crackleopts");
    let output = fixture.command(&helper, &[]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("setup"));
}

#[test]
fn help_and_debug_run_with_an_isolated_environment() {
    let fixture = Fixture::new("help");
    let output = fixture.run(&["help"]);
    assert!(output.status.success());
    let help = stdout(&output);
    assert!(help.contains("Usage: crackle <command> [arguments]"));
    assert!(help.contains("Setup and updates"));
    assert!(help.contains("`check-update` only reports"));
    assert!(!help.contains('\t'));

    let output = fixture.run(&["debug"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("CRACKLE CONFIG"));
    assert!(text.contains("PKG_PREFIX:"));
    assert!(text.contains("Dir::State::status="));
}

#[test]
fn no_arguments_show_help_without_panicking() {
    let fixture = Fixture::new("no-args");
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("Usage: crackle <command> [arguments]"));
}

#[test]
fn clean_removes_owned_state_and_keeps_unrelated_apt_dirs() {
    let fixture = Fixture::new("clean");
    fixture.recording_stub("nix-collect-garbage", 0);
    fixture.recording_stub("nix-store", 0);

    let pkg_path = fixture.root.join("data/crackle");
    fs::create_dir_all(pkg_path.join("packages/xz-utils")).unwrap();
    fs::write(pkg_path.join(".crackle-owned"), "crackle\n").unwrap();

    let owned_cache = fixture.root.join("cache/apt");
    fs::create_dir_all(owned_cache.join("archives")).unwrap();
    fs::write(owned_cache.join(".crackle-owned"), "crackle\n").unwrap();

    let unrelated = fixture.root.join("config/apt");
    fs::create_dir_all(&unrelated).unwrap();
    fs::write(unrelated.join("sources.list"), "keep").unwrap();

    let output = fixture.run(&["clean"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!pkg_path.exists());
    assert!(!owned_cache.exists());
    assert!(unrelated.join("sources.list").is_file());
    let calls = fixture.calls();
    assert!(calls.contains("nix-collect-garbage -d"), "{calls}");
    assert!(calls.contains("nix-store --optimise"), "{calls}");
}

#[test]
fn install_rewrites_the_config_and_creates_a_backup() {
    let fixture = Fixture::new("install");
    fixture.recording_stub("home-manager", 0);
    let config = fixture.write_config("{\n  home.packages = [\n    git\n  ];\n}\n");

    let output = fixture.run(&["install", "vim"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let updated = fs::read_to_string(&config).unwrap();
    assert!(updated.contains("    pkgs.vim\n    git\n"), "{updated}");
    let backup = fs::read_to_string(format!("{}.backup", config.display())).unwrap();
    assert!(backup.contains("    git\n"));
    let calls = fixture.calls();
    assert!(
        calls.contains("home-manager build --no-out-link"),
        "{calls}"
    );
    assert!(calls.contains("home-manager switch"), "{calls}");
}

#[test]
fn failed_build_restores_the_backup_and_exits_nonzero() {
    let fixture = Fixture::new("rollback");
    fixture.stub(
        "home-manager",
        "if [ \"$1\" = build ]; then echo 'error: broken package' >&2; exit 1; fi\nexit 0",
    );
    let original = "{\n  home.packages = [\n    git\n  ];\n}\n";
    let config = fixture.write_config(original);

    let output = fixture.run(&["remove", "git"]);
    assert_eq!(output.status.code(), Some(100));
    assert!(stderr(&output).contains("home-manager build failed"));
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
}

#[test]
fn nix_manager_list_strips_only_the_pkgs_prefix() {
    let fixture = Fixture::new("list");
    fixture.write_config("{\n  home.packages = [\n    pkgs.vim\n    git\n  ];\n}\n");
    let manager = fixture.binary_as("nix-manager");
    let output = fixture.command(&manager, &["list"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "vim\ngit\n");
}

#[test]
fn nuke_is_best_effort_and_cleans_crackle_files() {
    let fixture = Fixture::new("nuke");
    fixture.recording_stub("nix-channel", 1);
    fixture.recording_stub("sudo", 0);
    let prefix = fixture.root.join("prefix");
    fs::create_dir_all(prefix.join("bin")).unwrap();
    fs::write(prefix.join("bin/crackle"), "old").unwrap();
    let completion = fixture
        .root
        .join("data/bash-completion/completions/crackle");
    fs::create_dir_all(completion.parent().unwrap()).unwrap();
    fs::write(&completion, "old").unwrap();
    let unrelated = fixture.home.join(".config/home-manager/unrelated.txt");
    fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
    fs::write(&unrelated, "keep").unwrap();

    let output = fixture.run(&["nuke"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!prefix.join("bin/crackle").exists());
    assert!(!completion.exists());
    assert!(!unrelated.exists(), "home-manager state is removed");
}
