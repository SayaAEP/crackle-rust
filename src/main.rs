mod cleanup;
mod cli;
mod config;
mod helpers;
mod installer;
mod maintenance;
mod migration;
mod packages;
mod setup;
mod system;
mod update;

#[cfg(test)]
use cli::help_text;
use cli::{COMMANDS, dispatch, show_help};
use helpers::mount_option;
use helpers::{debug, fallback_triplet, helper, sudo_debug};
#[cfg(test)]
use helpers::{desktop_entry, legacy_triplet_from_bash, mount_option_from, sudo_debug_output};
#[cfg(test)]
use installer::install_binaries_at;
use installer::{install_binaries, install_binary_as, write_completion};
use maintenance::{
    check_interrupted, install_signal_handlers, migrate, nuke, remount_root, rollback,
    sudo_command, sudo_link,
};
#[cfg(test)]
use maintenance::{interruption_result, mount_restore_option};
#[cfg(test)]
use packages::*;
use packages::{click, crack};
use setup::*;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
use system::*;
#[cfg(test)]
use update::{
    Channel, ReleaseMetadata, UpdateDecision, compare_versions, decide_update,
    parse_release_metadata, parse_version_segments, upgrade_binaries_at,
};
use update::{check_update, upgrade, upgrade_install, version};

const NIX_VERSION: &str = "26.05";
const CRACKLE_BIN_ARCHIVE: &str =
    "https://gitlab.com/tuxecure/crackle-apt/cracklebin/-/archive/master/cracklebin-master.tar.gz";
#[cfg(unix)]
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
const COMMAND_NAMES: &[&str] = &[
    "crackle",
    "arch_triplet",
    "desktop_trunc",
    "_crackleopts",
    "remount",
    "rorw",
    "nix-manager",
    "osvariant",
    "rwnix",
];

const CRACKLE_VERSION: &str = env!("CRACKLE_VERSION");
const CRACKLE_CHANNEL: &str = env!("CRACKLE_CHANNEL");
const CRACKLE_COMMIT: &str = env!("CRACKLE_COMMIT");
const CRACKLE_BUILT: &str = env!("CRACKLE_BUILT");
const CRACKLE_TARGET: &str = env!("CRACKLE_TARGET");
const RELEASE_BASE_NIGHTLY: &str =
    "https://github.com/SayaAEP/crackle-rust/releases/download/nightly";
const RELEASE_BASE_STABLE: &str =
    "https://github.com/SayaAEP/crackle-rust/releases/latest/download";
const RELEASE_BINARY: &str = "crackle-aarch64-unknown-linux-gnu";

fn main() {
    install_signal_handlers();
    if let Err(error) = dispatch() {
        eprintln!("Crackle: {error}");
        std::process::exit(100);
    }
}

fn short_commit() -> String {
    if CRACKLE_COMMIT == "unknown" {
        return "unknown".to_owned();
    }
    CRACKLE_COMMIT.chars().take(7).collect()
}

#[cfg(test)]
#[path = "tests/main_tests.rs"]
mod tests;
