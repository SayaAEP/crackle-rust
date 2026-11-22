mod cleanup;
mod config;
mod migration;

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommandGroup {
    Setup,
    Packages,
    Touch,
    Maintenance,
    Migration,
}

impl CommandGroup {
    fn title(self) -> &'static str {
        match self {
            Self::Setup => "Setup and updates",
            Self::Packages => "Packages",
            Self::Touch => "Ubuntu Touch",
            Self::Maintenance => "Maintenance",
            Self::Migration => "Migration",
        }
    }
}

struct CommandSpec {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
    group: CommandGroup,
}

/// One table for help, completion, and tests so they cannot drift.
const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "setup",
        usage: "setup",
        summary: "Install/configure Nix and home-manager",
        group: CommandGroup::Setup,
    },
    CommandSpec {
        name: "update",
        usage: "update",
        summary: "Update Nix channels and activate home-manager",
        group: CommandGroup::Setup,
    },
    CommandSpec {
        name: "version",
        usage: "version",
        summary: "Show version, channel, commit, and target",
        group: CommandGroup::Setup,
    },
    CommandSpec {
        name: "check-update",
        usage: "check-update [chan]",
        summary: "Check for a newer nightly or stable build",
        group: CommandGroup::Setup,
    },
    CommandSpec {
        name: "upgrade",
        usage: "upgrade [chan]",
        summary: "Update the Crackle binary (nightly or stable)",
        group: CommandGroup::Setup,
    },
    CommandSpec {
        name: "install",
        usage: "install PKG...",
        summary: "Add home-manager packages and activate",
        group: CommandGroup::Packages,
    },
    CommandSpec {
        name: "remove",
        usage: "remove PKG...",
        summary: "Remove home-manager packages and activate",
        group: CommandGroup::Packages,
    },
    CommandSpec {
        name: "list",
        usage: "list",
        summary: "List installed home-manager packages",
        group: CommandGroup::Packages,
    },
    CommandSpec {
        name: "clean",
        usage: "clean",
        summary: "Garbage-collect Nix and clean Crackle state",
        group: CommandGroup::Packages,
    },
    CommandSpec {
        name: "crack",
        usage: "crack PKG",
        summary: "Download a package and extract it under ~/packages",
        group: CommandGroup::Touch,
    },
    CommandSpec {
        name: "click",
        usage: "click PKG...",
        summary: "Build a Click package",
        group: CommandGroup::Touch,
    },
    CommandSpec {
        name: "bump-version",
        usage: "bump-version PKG...",
        summary: "Update a Click manifest version from apt",
        group: CommandGroup::Touch,
    },
    CommandSpec {
        name: "sudo",
        usage: "sudo BIN...",
        summary: "Link binaries for `sudo -i`",
        group: CommandGroup::Touch,
    },
    CommandSpec {
        name: "sudok",
        usage: "sudok BIN...",
        summary: "Remove those links",
        group: CommandGroup::Touch,
    },
    CommandSpec {
        name: "debug",
        usage: "debug",
        summary: "Print paths and configuration",
        group: CommandGroup::Maintenance,
    },
    CommandSpec {
        name: "nuke",
        usage: "nuke",
        summary: "Remove Crackle, Nix, and home-manager state",
        group: CommandGroup::Maintenance,
    },
    CommandSpec {
        name: "help",
        usage: "help",
        summary: "Show this help",
        group: CommandGroup::Maintenance,
    },
    CommandSpec {
        name: "migrate",
        usage: "migrate",
        summary: "Replace legacy Crackle with the Rust build",
        group: CommandGroup::Migration,
    },
    CommandSpec {
        name: "rollback",
        usage: "rollback [ID]",
        summary: "Restore a migration backup",
        group: CommandGroup::Migration,
    },
];

fn main() {
    install_signal_handlers();
    if let Err(error) = dispatch() {
        eprintln!("Crackle: {error}");
        std::process::exit(100);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstallLayout {
    /// The normal install: `bin/crackle` plus all helper names and completion.
    Standard,
    /// Coexists with a legacy Bash/Python Crackle installation: installs
    /// `bin/crackle-rust`, only fills missing helpers, and uses isolated paths.
    SideBySide,
}

impl InstallLayout {
    fn main_name(self) -> &'static str {
        match self {
            Self::Standard => "crackle",
            Self::SideBySide => "crackle-rust",
        }
    }

    fn data_dir_name(self) -> &'static str {
        match self {
            Self::Standard => "crackle",
            Self::SideBySide => "crackle-rust",
        }
    }

    fn is_side_by_side(self) -> bool {
        matches!(self, Self::SideBySide)
    }
}

fn invoked_name() -> String {
    let name = env::args_os()
        .next()
        .and_then(|path| Path::new(&path).file_name().map(|name| name.to_owned()))
        .and_then(|name| name.to_str().map(str::to_owned))
        .unwrap_or_else(|| "crackle".to_owned());
    name.strip_suffix(".exe").unwrap_or(&name).to_owned()
}

fn legacy_install_present(prefix: &Path) -> bool {
    for name in [
        "basefunc",
        "clickfunc",
        "setupfunc",
        "upgradefunc",
        "sudofunc",
    ] {
        if prefix.join("lib").join(name).is_file() {
            return true;
        }
    }
    let crackle = prefix.join("bin/crackle");
    let Ok(mut file) = fs::File::open(&crackle) else {
        return false;
    };
    let mut magic = [0u8; 2];
    file.read_exact(&mut magic).is_ok() && &magic == b"#!"
}

fn resolve_layout_from(
    invoked: &str,
    legacy_present: bool,
    override_value: Option<&str>,
) -> InstallLayout {
    match override_value {
        Some("side-by-side") => InstallLayout::SideBySide,
        Some("standard") => InstallLayout::Standard,
        _ if legacy_present || invoked == "crackle-rust" => InstallLayout::SideBySide,
        _ => InstallLayout::Standard,
    }
}

fn resolve_layout(invoked: &str) -> io::Result<InstallLayout> {
    let override_value = env::var("CRACKLE_INSTALL_LAYOUT").ok();
    let legacy_present = legacy_install_present(&prefix()?);
    Ok(resolve_layout_from(
        invoked,
        legacy_present,
        override_value.as_deref(),
    ))
}

fn library_dir(layout: InstallLayout, prefix: &Path) -> PathBuf {
    prefix.join("lib").join(layout.data_dir_name())
}

fn nuke_allowed(layout: InstallLayout, legacy_present: bool, forced: bool) -> io::Result<()> {
    if layout.is_side_by_side() && legacy_present && !forced {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "legacy Crackle is installed; nuke removes shared Nix/home-manager state. Re-run with CRACKLE_FORCE=1 to force a full nuke.",
        ));
    }
    Ok(())
}

fn apt_path(
    layout: InstallLayout,
    variable: &str,
    xdg_variable: &str,
    fallback: &str,
    subdirectory: &str,
) -> io::Result<PathBuf> {
    if let Some(path) = env::var_os(variable) {
        return Ok(PathBuf::from(path));
    }
    let base = xdg_path(xdg_variable, fallback)?;
    if layout.is_side_by_side() {
        Ok(base.join(layout.data_dir_name()).join(subdirectory))
    } else {
        Ok(base.join(subdirectory))
    }
}

fn dispatch() -> io::Result<()> {
    check_interrupted()?;
    let mut args: Vec<OsString> = env::args_os().collect();
    let executable = invoked_name();
    args.remove(0);
    if executable == "nix-manager" {
        return nix_manager(&args);
    }
    if COMMAND_NAMES.contains(&executable.as_str()) && executable != "crackle" {
        return helper(&executable, &args);
    }

    // Legacy `crackle:26-30`: the main entry point refuses to run as root unless
    // ROOT is set, including for help/debug/list. Helper modes and `nix-manager`
    // are separate legacy scripts and stay exempt.
    if is_root() && env::var_os("ROOT").is_none() {
        println!("Crackle: please don't run me as root");
        std::process::exit(100);
    }

    let command = args
        .first()
        .and_then(|value| value.to_str())
        .unwrap_or("help");
    let values = args.get(1..).unwrap_or(&[]);
    if command == "-h" || command == "--help" {
        return show_help();
    }
    if command == "-V" || command == "--version" {
        return version();
    }
    match command {
        "help" => show_help(),
        "debug" => debug(),
        "setup" => setup(),
        "version" => version(),
        "check-update" => check_update(values),
        "upgrade" => upgrade(values),
        "__upgrade" => upgrade_install(),
        "migrate" => migrate(),
        "rollback" => rollback(values.first().and_then(|value| value.to_str())),
        "clean" => clean(),
        "update" => update(),
        "install" | "remove" | "list" => {
            let mut forwarded = vec![OsString::from(command)];
            forwarded.extend_from_slice(values);
            nix_manager(&forwarded)
        }
        "crack" => crack(values),
        "click" => click(values, false),
        "bump-version" => click(values, true),
        "sudo" if values.first().is_some_and(|value| value == "debug") => sudo_debug(),
        "sudok" if values.first().is_some_and(|value| value == "debug") => sudo_debug(),
        "sudo" => sudo_link(values, true),
        "sudok" => sudo_link(values, false),
        "nuke" => nuke(),
        _ => show_help(),
    }
}

fn home() -> io::Result<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "HOME (or USERPROFILE on Windows) is not set",
            )
        })
}

fn xdg_path(variable: &str, fallback: &str) -> io::Result<PathBuf> {
    if let Some(path) = env::var_os(variable) {
        return Ok(PathBuf::from(path));
    }
    Ok(home()?.join(fallback))
}

fn prefix() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("PKG_PREFIX") {
        Ok(PathBuf::from(path))
    } else {
        Ok(home()?.join(".local"))
    }
}

fn state_dir(layout: InstallLayout) -> io::Result<PathBuf> {
    Ok(xdg_path("XDG_STATE_HOME", ".local/state")?.join(layout.data_dir_name()))
}

fn completion_dir() -> io::Result<PathBuf> {
    Ok(xdg_path("XDG_DATA_HOME", ".local/share")?.join("bash-completion/completions"))
}

fn completion_file() -> io::Result<PathBuf> {
    Ok(completion_dir()?.join("crackle"))
}

fn pkg_path(layout: InstallLayout) -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("PKG_PATH") {
        Ok(PathBuf::from(path))
    } else {
        Ok(xdg_path("XDG_DATA_HOME", ".local/share")?.join(layout.data_dir_name()))
    }
}

fn command_output(program: &str, args: &[&str]) -> io::Result<Output> {
    capture_command(command(program).args(args))
}

fn run(program: &str, args: &[&str]) -> io::Result<()> {
    let status = run_command(command(program).args(args))?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "`{program} {}` exited with {status}",
            args.join(" ")
        )))
    }
}

fn run_os(program: &str, args: &[OsString]) -> io::Result<()> {
    let status = run_command(command(program).args(args))?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "`{program}` exited with {status}"
        )))
    }
}

fn run_command(command: &mut Command) -> io::Result<std::process::ExitStatus> {
    let status = command.status()?;
    check_interrupted()?;
    Ok(status)
}

fn run_cleanup(program: &str, args: &[&str]) -> io::Result<()> {
    let status = command(program).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{program} {} failed",
            args.join(" ")
        )))
    }
}

fn capture_command(command: &mut Command) -> io::Result<Output> {
    let output = command.output()?;
    check_interrupted()?;
    Ok(output)
}

fn available(program: &str) -> bool {
    executable_path(program).is_some()
}

/// Search order mirrors legacy `check_path`: Crackle's own bin and the Nix
/// profile come first, then PATH. This prevents a broken `/snap/bin` tool from
/// shadowing a working or freshly extracted one.
fn search_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Ok(prefix) = prefix() {
        directories.push(prefix.join("bin"));
    }
    if let Ok(home) = home() {
        directories.push(home.join(".local/bin"));
        directories.push(home.join(".nix-profile/bin"));
        directories.push(PathBuf::from("/nix/var/nix/profiles/default/bin"));
    }
    directories.extend(
        ["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
            .into_iter()
            .map(PathBuf::from),
    );
    directories.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    directories
}

fn executable_path(program: &str) -> Option<PathBuf> {
    search_directories()
        .into_iter()
        .map(|directory| directory.join(program))
        .find(|path| path.is_file())
}

fn effective_path() -> OsString {
    env::join_paths(search_directories())
        .unwrap_or_else(|_| env::var_os("PATH").unwrap_or_default())
}

fn curl_candidates() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = search_directories()
        .into_iter()
        .map(|directory| directory.join("curl"))
        .collect();
    candidates.extend(
        [
            "/usr/bin/curl",
            "/bin/curl",
            "/usr/local/bin/curl",
            "/snap/bin/curl",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    candidates
}

/// Prefer a curl that actually runs. Ubuntu Touch can have a broken Snap curl
/// shadowing the system one.
fn working_curl() -> Option<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    for candidate in curl_candidates() {
        if !seen.insert(candidate.clone()) || !candidate.is_file() {
            continue;
        }
        if Command::new(&candidate)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            return Some(candidate);
        }
    }
    None
}

fn curl_available() -> bool {
    working_curl().is_some()
}

/// Put the directory of a working curl first so the Nix installer's internal
/// `curl` lookup cannot pick a broken Snap wrapper.
fn path_with_working_curl() -> OsString {
    let mut directories = Vec::new();
    if let Some(curl) = working_curl()
        && let Some(parent) = curl.parent()
    {
        directories.push(parent.to_path_buf());
    }
    directories.extend(env::split_paths(&effective_path()));
    env::join_paths(directories).unwrap_or_else(|_| effective_path())
}

pub(crate) fn curl_command() -> io::Result<Command> {
    let path = working_curl().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no working curl executable found (tried the prefix, /usr/bin/curl, /bin/curl, /usr/local/bin/curl, and PATH)",
        )
    })?;
    let mut command = Command::new(path);
    command.env("PATH", effective_path());
    Ok(command)
}

/// Download with a working curl, falling back to wget. Used for the Nix
/// installer, repository configuration, signing keys, and migration assets.
pub(crate) fn download_file(url: &str, destination: &Path) -> io::Result<()> {
    if !url.starts_with("https://") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "downloads require an HTTPS URL",
        ));
    }
    let curl_error = match curl_command() {
        Ok(mut curl) => match curl
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "15",
                "--max-time",
                "60",
                url,
                "--output",
            ])
            .arg(destination)
            .status()
        {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => format!("curl exited with {status}"),
            Err(error) => format!("could not run curl: {error}"),
        },
        Err(error) => format!("curl unavailable: {error}"),
    };
    remove_file_if_exists(destination).map_err(|error| {
        io::Error::other(format!(
            "could not remove incomplete download {}: {error}",
            destination.display()
        ))
    })?;
    if available("wget") {
        let result = command("wget")
            .args(["--https-only", "--timeout=60", "--tries=1", "-q", "-O"])
            .arg(destination)
            .arg(url)
            .status();
        match result {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => {
                return match remove_file_if_exists(destination) {
                    Ok(()) => Err(io::Error::other(format!(
                        "could not download {url}: wget exited with {status}"
                    ))),
                    Err(error) => Err(io::Error::other(format!(
                        "wget exited with {status}; could not remove partial download {}: {error}",
                        destination.display()
                    ))),
                };
            }
            Err(error) => {
                return match remove_file_if_exists(destination) {
                    Ok(()) => Err(io::Error::other(format!(
                        "could not download {url}: could not run wget: {error}"
                    ))),
                    Err(cleanup_error) => Err(io::Error::other(format!(
                        "could not run wget: {error}; could not remove partial download {}: {cleanup_error}",
                        destination.display()
                    ))),
                };
            }
        }
    }
    Err(io::Error::other(format!(
        "could not download {url}: {curl_error}; wget is unavailable"
    )))
}

fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn remove_dir_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

pub(crate) fn command(program: &str) -> Command {
    let executable = executable_path(program).unwrap_or_else(|| PathBuf::from(program));
    let mut command = Command::new(executable);
    command.env("PATH", effective_path());
    command
}

fn is_root() -> bool {
    command_output("id", &["-u"])
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
}

fn ensure_not_root() -> io::Result<()> {
    if is_root() && env::var_os("ROOT").is_none() {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "please don't run me as root",
        ))
    } else {
        Ok(())
    }
}

fn short_commit() -> String {
    if CRACKLE_COMMIT == "unknown" {
        return "unknown".to_owned();
    }
    CRACKLE_COMMIT.chars().take(7).collect()
}

fn help_text() -> String {
    let width = COMMANDS
        .iter()
        .map(|command| command.usage.len())
        .max()
        .unwrap_or(0);
    let mut text = format!(
        "crackle {} ({}, {}) - Nix home-manager on Ubuntu Touch\n\nUsage: crackle <command> [arguments]\n",
        CRACKLE_VERSION,
        CRACKLE_CHANNEL,
        short_commit()
    );
    for group in [
        CommandGroup::Setup,
        CommandGroup::Packages,
        CommandGroup::Touch,
        CommandGroup::Maintenance,
        CommandGroup::Migration,
    ] {
        text.push('\n');
        text.push_str(group.title());
        text.push('\n');
        for command in COMMANDS.iter().filter(|command| command.group == group) {
            text.push_str(&format!(
                "  {:<width$}  {}\n",
                command.usage,
                command.summary,
                width = width
            ));
        }
    }
    text.push_str(
        "\n`update` refreshes Nix channels; `upgrade` updates the Crackle binary itself; `check-update` only reports.\n\
         Installed helpers (run directly): arch_triplet, desktop_trunc, _crackleopts, nix-manager, osvariant, remount, rorw, rwnix\n",
    );
    text
}

fn show_help() -> io::Result<()> {
    print!("{}", help_text());
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    Nightly,
    Stable,
}

impl Channel {
    fn name(self) -> &'static str {
        match self {
            Self::Nightly => "nightly",
            Self::Stable => "stable",
        }
    }

    fn base_url(self) -> &'static str {
        match self {
            Self::Nightly => RELEASE_BASE_NIGHTLY,
            Self::Stable => RELEASE_BASE_STABLE,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "nightly" => Some(Self::Nightly),
            "stable" => Some(Self::Stable),
            _ => None,
        }
    }
}

fn installed_channel() -> Option<Channel> {
    Channel::parse(CRACKLE_CHANNEL)
}

fn layout_name(layout: InstallLayout) -> &'static str {
    if layout.is_side_by_side() {
        "side-by-side"
    } else {
        "standard"
    }
}

fn short(value: &str) -> String {
    value.chars().take(7).collect()
}

fn version() -> io::Result<()> {
    println!(
        "crackle {} ({}, {})",
        CRACKLE_VERSION,
        CRACKLE_CHANNEL,
        short_commit()
    );
    println!("commit {}  built {}", CRACKLE_COMMIT, CRACKLE_BUILT);
    println!("target {}", CRACKLE_TARGET);
    Ok(())
}

struct ReleaseMetadata {
    channel: String,
    version: String,
    tag: String,
    commit: String,
    built: String,
}

fn parse_release_metadata(text: &str) -> Option<ReleaseMetadata> {
    if text.len() > 4096 || text.lines().count() > 9 {
        return None;
    }
    let mut channel = None;
    let mut version = None;
    let mut tag = None;
    let mut commit = None;
    let mut built = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let (key, value) = line.split_once('=')?;
        let value = value.trim().to_owned();
        let destination = match key.trim() {
            "channel" => Some(&mut channel),
            "version" => Some(&mut version),
            "tag" => Some(&mut tag),
            "commit" => Some(&mut commit),
            "built" => Some(&mut built),
            _ => None,
        };
        if let Some(destination) = destination
            && destination.replace(value).is_some()
        {
            return None;
        }
    }
    let channel = channel?;
    let version = version?;
    let tag = tag?;
    let commit = commit?;
    let built = built?;
    if !matches!(channel.as_str(), "nightly" | "stable")
        || version.is_empty()
        || tag.is_empty()
        || commit.len() != 40
        || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        || built.is_empty()
        || (channel == "stable" && parse_version_segments(&version).is_none())
        || (channel == "stable"
            && tag != version
            && tag.strip_prefix('v') != Some(version.as_str()))
        || (channel == "nightly" && tag != "nightly")
    {
        return None;
    }
    Some(ReleaseMetadata {
        channel,
        version,
        tag,
        commit,
        built,
    })
}

fn parse_version_segments(version: &str) -> Option<Vec<u64>> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let core = version.split(['-', '+']).next()?;
    if core.is_empty() {
        return None;
    }
    let mut segments = Vec::new();
    for part in core.split('.') {
        if part.is_empty() {
            return None;
        }
        if !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        segments.push(part.parse().ok()?);
    }
    Some(segments)
}

fn compare_versions(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
    for index in 0..left.len().max(right.len()) {
        let left = left.get(index).copied().unwrap_or(0);
        let right = right.get(index).copied().unwrap_or(0);
        match left.cmp(&right) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdateDecision {
    Current,
    UpdateAvailable,
    ChannelMismatch,
    Unknown,
}

fn decide_update(
    channel: Channel,
    installed_channel: &str,
    installed_version: &str,
    installed_commit: &str,
    remote: &ReleaseMetadata,
) -> UpdateDecision {
    if installed_channel != channel.name() {
        return UpdateDecision::ChannelMismatch;
    }
    if installed_commit == "unknown" {
        return UpdateDecision::Unknown;
    }
    match channel {
        Channel::Nightly => {
            if remote.commit.eq_ignore_ascii_case(installed_commit) {
                UpdateDecision::Current
            } else {
                UpdateDecision::UpdateAvailable
            }
        }
        Channel::Stable => {
            let Some(local) = parse_version_segments(installed_version) else {
                return UpdateDecision::Unknown;
            };
            let Some(remote_version) = parse_version_segments(&remote.version) else {
                return UpdateDecision::Unknown;
            };
            if compare_versions(&remote_version, &local) == std::cmp::Ordering::Greater {
                UpdateDecision::UpdateAvailable
            } else {
                UpdateDecision::Current
            }
        }
    }
}

fn installed_label() -> String {
    match installed_channel() {
        Some(Channel::Stable) => CRACKLE_VERSION.to_owned(),
        Some(Channel::Nightly) => short_commit(),
        None => format!("{} ({})", CRACKLE_VERSION, short_commit()),
    }
}

fn remote_label(channel: Channel, remote: &ReleaseMetadata) -> String {
    match channel {
        Channel::Stable => {
            if remote.tag.is_empty() {
                remote.version.clone()
            } else {
                format!("{} ({})", remote.version, remote.tag)
            }
        }
        Channel::Nightly => short(&remote.commit),
    }
}

fn resolve_channel_arg(value: Option<&str>) -> Result<Channel, String> {
    let name = match value {
        Some(value) => value.to_owned(),
        None => match env::var_os("CRACKLE_CHANNEL") {
            Some(value) => value
                .into_string()
                .map_err(|_| "CRACKLE_CHANNEL is not valid UTF-8".to_owned())?,
            None => CRACKLE_CHANNEL.to_owned(),
        },
    };
    match name.as_str() {
        "nightly" => Ok(Channel::Nightly),
        "stable" => Ok(Channel::Stable),
        "dev" => Ok(Channel::Nightly),
        other => Err(format!(
            "unknown channel '{other}' (expected nightly or stable)"
        )),
    }
}

fn command_channel(values: &[OsString]) -> Result<Channel, String> {
    if values.len() > 1 {
        return Err("expected at most one channel argument".to_owned());
    }
    let value = match values.first() {
        Some(value) => Some(
            value
                .to_str()
                .ok_or_else(|| "channel argument is not valid UTF-8".to_owned())?,
        ),
        None => None,
    };
    resolve_channel_arg(value)
}

fn release_metadata_url(channel: Channel) -> String {
    format!("{}/version.txt", channel.base_url())
}

fn fetch_release_metadata(channel: Channel, layout: InstallLayout) -> io::Result<ReleaseMetadata> {
    let directory = state_dir(layout)?.join(format!("update-check-{}", std::process::id()));
    fs::create_dir_all(&directory)?;
    let path = directory.join("version.txt");
    let result = download_file(&release_metadata_url(channel), &path).and_then(|()| {
        if fs::metadata(&path)?.len() > 4096 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "version.txt is too large",
            ));
        }
        let text = fs::read_to_string(&path)?;
        let metadata = parse_release_metadata(&text).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "version.txt is malformed")
        })?;
        if metadata.channel != channel.name() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "version.txt channel does not match the requested release",
            ));
        }
        Ok(metadata)
    });
    match (result, remove_dir_if_exists(&directory)) {
        (Ok(metadata), Ok(())) => Ok(metadata),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup_error)) => Err(io::Error::other(format!(
            "{error}; could not remove temporary metadata directory: {cleanup_error}"
        ))),
    }
}

fn exit_with(code: i32) -> ! {
    let _ = io::stdout().flush();
    std::process::exit(code)
}

fn check_update(values: &[OsString]) -> io::Result<()> {
    let channel = match command_channel(values) {
        Ok(channel) => channel,
        Err(error) => {
            eprintln!("Crackle: {error}");
            exit_with(2)
        }
    };
    let layout = match resolve_layout(&invoked_name()) {
        Ok(layout) => layout,
        Err(error) => {
            eprintln!("Crackle: could not check updates: {error}");
            exit_with(11)
        }
    };
    let remote = match fetch_release_metadata(channel, layout) {
        Ok(remote) => remote,
        Err(error) => {
            eprintln!("Crackle: could not check updates: {error}");
            exit_with(11)
        }
    };
    println!("Channel: {}", channel.name());
    println!(
        "Installed: {} (commit {}, built {})",
        installed_label(),
        short(CRACKLE_COMMIT),
        CRACKLE_BUILT
    );
    println!(
        "Latest:    {} (commit {}, built {})",
        remote_label(channel, &remote),
        short(&remote.commit),
        remote.built
    );
    let code = match decide_update(
        channel,
        CRACKLE_CHANNEL,
        CRACKLE_VERSION,
        CRACKLE_COMMIT,
        &remote,
    ) {
        UpdateDecision::Current => {
            println!("Crackle {} is current.", installed_label());
            0
        }
        UpdateDecision::UpdateAvailable => {
            println!("Update available. Run `crackle upgrade`.");
            10
        }
        UpdateDecision::ChannelMismatch => {
            println!(
                "Installed channel is {}; requested {}. Run `crackle upgrade {}` to switch.",
                CRACKLE_CHANNEL,
                channel.name(),
                channel.name()
            );
            12
        }
        UpdateDecision::Unknown => {
            eprintln!("Crackle: could not determine the installed version.");
            11
        }
    };
    exit_with(code)
}

fn expected_digest(path: &Path, asset: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(digest) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else {
            continue;
        };
        if name.trim_start_matches([' ', '*']) == asset {
            return Some(digest.to_owned());
        }
    }
    None
}

fn upgrade(values: &[OsString]) -> io::Result<()> {
    let channel = match command_channel(values) {
        Ok(channel) => channel,
        Err(error) => {
            eprintln!("Crackle: {error}");
            exit_with(2)
        }
    };
    if let Err(error) = perform_upgrade(channel) {
        eprintln!("Crackle: upgrade failed: {error}");
        exit_with(11);
    }
    Ok(())
}

fn perform_upgrade(channel: Channel) -> io::Result<()> {
    let layout = resolve_layout(&invoked_name())?;
    let remote = match fetch_release_metadata(channel, layout) {
        Ok(remote) => remote,
        Err(error) => {
            return Err(io::Error::other(format!(
                "could not fetch release metadata: {error}"
            )));
        }
    };
    let switching = CRACKLE_CHANNEL != channel.name();
    let forced = env::var("CRACKLE_UPGRADE_FORCE").ok().as_deref() == Some("1");
    if !switching && !forced {
        match decide_update(
            channel,
            CRACKLE_CHANNEL,
            CRACKLE_VERSION,
            CRACKLE_COMMIT,
            &remote,
        ) {
            UpdateDecision::Current => {
                println!(
                    "crackle {} is already current ({}).",
                    installed_label(),
                    remote_label(channel, &remote)
                );
                return Ok(());
            }
            UpdateDecision::UpdateAvailable => {}
            UpdateDecision::ChannelMismatch => {}
            UpdateDecision::Unknown => {
                return Err(io::Error::other(
                    "could not determine the installed version; set CRACKLE_UPGRADE_FORCE=1 to reinstall",
                ));
            }
        }
    }

    let base = channel.base_url();
    let bin = prefix()?.join("bin");
    fs::create_dir_all(&bin)?;
    let directory = state_dir(layout)?.join(format!("upgrade-{}", std::process::id()));
    fs::create_dir_all(&directory)?;
    let binary = directory.join(RELEASE_BINARY);
    let sums = directory.join("SHA256SUMS");
    let staged = bin.join(format!(".crackle-upgrade-{}", std::process::id()));
    let result = (|| -> io::Result<()> {
        download_file(&format!("{base}/{RELEASE_BINARY}"), &binary)?;
        download_file(&format!("{base}/SHA256SUMS"), &sums)?;
        let expected = expected_digest(&sums, RELEASE_BINARY).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "release checksum is missing")
        })?;
        let actual = migration::checksum(&binary)?;
        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "downloaded binary failed SHA-256 verification",
            ));
        }

        fs::copy(&binary, &staged)?;
        set_executable(&staged)?;
        let mut command = Command::new(&staged);
        command
            .arg("__upgrade")
            .env("PATH", effective_path())
            .env("CRACKLE_INSTALL_LAYOUT", layout_name(layout));
        let status = run_command(&mut command);
        match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(io::Error::other(format!(
                "new binary failed to install: {status}"
            ))),
            Err(error) => Err(error),
        }
    })();
    let staged_cleanup = remove_file_if_exists(&staged);
    let directory_cleanup = remove_dir_if_exists(&directory);
    let cleanup = match (staged_cleanup, directory_cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(io::Error::other(format!("{first}; {second}"))),
    };
    match (result, cleanup) {
        (Ok(()), Ok(())) => {}
        (Err(error), Ok(())) => return Err(error),
        (Ok(()), Err(error)) => return Err(error),
        (Err(error), Err(cleanup_error)) => {
            return Err(io::Error::other(format!(
                "{error}; upgrade cleanup failed: {cleanup_error}"
            )));
        }
    }
    println!(
        "Upgraded to {} {} (commit {}).",
        channel.name(),
        remote.version,
        short(&remote.commit)
    );
    println!("Run `crackle setup` if Nix/home-manager still needs configuring.");
    Ok(())
}

fn upgrade_install() -> io::Result<()> {
    let layout = resolve_layout(&invoked_name())?;
    let prefix = prefix()?;
    let executable = env::current_exe()?;
    let completion = if layout.is_side_by_side() {
        None
    } else {
        Some(completion_file()?)
    };
    upgrade_binaries_at(layout, &prefix, &executable, completion.as_deref())
}

fn is_native_executable(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_err() {
        return false;
    }
    &magic == b"\x7fELF" || &magic[..2] == b"MZ"
}

fn upgrade_binaries_at(
    layout: InstallLayout,
    prefix: &Path,
    executable: &Path,
    completion: Option<&Path>,
) -> io::Result<()> {
    let bin = prefix.join("bin");
    fs::create_dir_all(&bin)?;
    match layout {
        InstallLayout::Standard => {
            for name in COMMAND_NAMES {
                install_binary_as(&bin, name, executable)?;
            }
            if let Some(completion) = completion {
                write_completion(completion)?;
            }
        }
        InstallLayout::SideBySide => {
            install_binary_as(&bin, layout.main_name(), executable)?;
            for name in COMMAND_NAMES
                .iter()
                .copied()
                .filter(|name| *name != "crackle")
            {
                let target = bin.join(name);
                match fs::symlink_metadata(&target) {
                    Ok(metadata)
                        if (metadata.is_file() || metadata.file_type().is_symlink())
                            && is_native_executable(&target) =>
                    {
                        install_binary_as(&bin, name, executable)?;
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        install_binary_as(&bin, name, executable)?;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(())
}

fn debug() -> io::Result<()> {
    let layout = resolve_layout(&invoked_name())?;
    let home = home()?;
    let pkg = pkg_path(layout)?;
    let prefix = prefix()?;
    println!("CRACKLE CONFIG");
    println!(
        "LAYOUT: '{}'",
        if layout.is_side_by_side() {
            "side-by-side"
        } else {
            "standard"
        }
    );
    println!("PKG_PATH: '{}'", pkg.display());
    println!("PKG_PREFIX: '{}'", prefix.display());
    println!("PKGS_DIR: '{}'", pkg.display());
    let completion = if layout.is_side_by_side() {
        "none (side-by-side)".to_owned()
    } else {
        completion_file()?.display().to_string()
    };
    println!("CRACKLECMP: '{completion}'");
    println!("CRACKDIR: '{}'", home.join("packages").display());
    println!("APT CONFIG");
    let apt_config = apt_path(layout, "APTCONFIG", "XDG_CONFIG_HOME", ".config", "apt")?;
    let apt_cache = apt_path(layout, "APTCACHE", "XDG_CACHE_HOME", ".cache", "apt")?;
    let apt_state = apt_path(layout, "APTSTATE", "XDG_STATE_HOME", ".local/state", "apt")?;
    let dpkg_state = apt_path(
        layout,
        "DPKGSTATE",
        "XDG_STATE_HOME",
        ".local/state",
        "dpkg",
    )?;
    println!("APTCONFIG: '{}'", apt_config.display());
    println!("APTCACHE: '{}'", apt_cache.display());
    println!("APTSTATE: '{}'", apt_state.display());
    println!(
        "APT_GET: apt-get -o Dir::Cache={} -o Dir::State={} -o Dir::State::status={} -o Dir::Etc={}",
        apt_cache.display(),
        apt_state.display(),
        dpkg_state.join("status").display(),
        apt_config.display()
    );
    println!(
        "APT_CACHE: apt-cache -o Dir::Cache={} -o Dir::State={} -o Dir::Etc={}",
        apt_cache.display(),
        apt_state.display(),
        apt_config.display()
    );
    println!("DPKGSTATE: '{}'", dpkg_state.display());
    Ok(())
}

fn sudo_debug() -> io::Result<()> {
    print!("{}", sudo_debug_output());
    Ok(())
}

fn sudo_debug_output() -> &'static str {
    "SUDO CONFIG\nSUDO_CONFIG: '/root/.config'\nSUDO_BIN: '/root/.local/bin'\nSUDO_STATE: '/root/.local/state'\n"
}

fn helper(name: &str, args: &[OsString]) -> io::Result<()> {
    match name {
        "rorw" => {
            if let Some(status) = mount_option(Path::new("/"))? {
                println!("{status}");
            }
        }
        "rwnix" => {
            if let Some(status) = mount_option(Path::new("/nix"))? {
                println!("{status}");
            }
        }
        "osvariant" => {
            // Legacy-exact: every line containing `Touch`, raw value after the
            // first `=` (quotes included). On the target device: `Touch`.
            if let Ok(contents) = fs::read_to_string("/etc/os-release") {
                for line in contents.lines().filter(|line| line.contains("Touch")) {
                    if let Some((_, value)) = line.split_once('=') {
                        println!("{value}");
                    }
                }
            }
        }
        "arch_triplet" => {
            let output =
                capture_command(command("dpkg-architecture").args(["-qDEB_HOST_MULTIARCH"]));
            match output {
                Ok(output) if output.status.success() => {
                    print!("{}", String::from_utf8_lossy(&output.stdout));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
                _ => println!(
                    "{}",
                    legacy_triplet().unwrap_or_else(|| fallback_triplet().to_string())
                ),
            }
        }
        "desktop_trunc" => desktop_trunc(args)?,
        "_crackleopts" => println!(
            "{}",
            COMMANDS
                .iter()
                .map(|command| command.name)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        "remount" => remount_root()?,
        _ => show_help()?,
    }
    Ok(())
}

fn fallback_triplet() -> &'static str {
    match env::consts::ARCH {
        "aarch64" => "aarch64-linux-gnu",
        "x86_64" => "x86_64-linux-gnu",
        _ => env::consts::ARCH,
    }
}

/// Legacy `arch_triplet` read `bash --version` and intended to print the
/// architecture, OS and ABI fields. The awk field numbers misfire on modern
/// bash output, so this parses the parenthesised triplet and drops the vendor.
fn legacy_triplet() -> Option<String> {
    let output = capture_command(command("bash").arg("--version")).ok()?;
    if !output.status.success() {
        return None;
    }
    let first_line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .to_owned();
    legacy_triplet_from_bash(&first_line)
}

fn legacy_triplet_from_bash(line: &str) -> Option<String> {
    // The triplet is the last parenthesised group; the first one is the
    // release patch level in `5.2.21(1)-release`.
    let start = line.rfind('(')? + 1;
    let rest = &line[start..];
    let end = rest.find(')')?;
    let triplet = &rest[..end];
    let parts: Vec<&str> = triplet.split('-').collect();
    match parts.as_slice() {
        [arch, _vendor, os, abi] => Some(format!("{arch}-{os}-{abi}")),
        _ => Some(triplet.to_owned()),
    }
}

fn mount_option(mountpoint: &Path) -> io::Result<Option<&'static str>> {
    let mounts = fs::read_to_string("/proc/mounts")?;
    Ok(mount_option_from(&mounts, mountpoint))
}

fn mount_option_from(mounts: &str, mountpoint: &Path) -> Option<&'static str> {
    let target = mountpoint.to_string_lossy();
    for line in mounts.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 4 && fields[1] == target {
            if fields[2] == "rootfs" {
                continue;
            }
            return fields[3].split(',').find_map(|option| match option {
                "ro" => Some("ro"),
                "rw" => Some("rw"),
                _ => None,
            });
        }
    }
    None
}

fn desktop_trunc(args: &[OsString]) -> io::Result<()> {
    let path = args
        .first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing desktop file"))?;
    let mut contents = String::new();
    fs::File::open(path)?.read_to_string(&mut contents)?;
    print!("{}", desktop_entry(&contents));
    Ok(())
}

fn desktop_entry(contents: &str) -> String {
    let mut output = String::new();
    let mut in_entry = false;
    for line in contents.lines() {
        if line == "[Desktop Entry]" {
            in_entry = true;
        } else if in_entry && line.is_empty() {
            output.push('\n');
            break;
        }
        if in_entry {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

fn install_binary_as(bin: &Path, name: &str, executable: &Path) -> io::Result<()> {
    let target = bin.join(name);
    let temporary = bin.join(format!(".{name}.new"));
    fs::copy(executable, &temporary)?;
    set_executable(&temporary)?;
    fs::rename(temporary, target)
}

fn write_completion(completion: &Path) -> io::Result<()> {
    let content = include_str!("../share/bash-completion/completions/crackle");
    atomic_write(completion, content.as_bytes())
}

fn install_binaries(layout: InstallLayout) -> io::Result<()> {
    let executable = env::current_exe()?;
    let completion = if layout.is_side_by_side() {
        None
    } else {
        Some(completion_file()?)
    };
    install_binaries_at(layout, &prefix()?, &executable, completion.as_deref())
}

fn install_binaries_at(
    layout: InstallLayout,
    prefix: &Path,
    executable: &Path,
    completion: Option<&Path>,
) -> io::Result<()> {
    let bin = prefix.join("bin");
    fs::create_dir_all(&bin)?;
    match layout {
        InstallLayout::Standard => {
            for name in COMMAND_NAMES {
                install_binary_as(&bin, name, executable)?;
            }
            if let Some(completion) = completion {
                write_completion(completion)?;
            }
            println!("Installed Crackle commands in {}.", bin.display());
        }
        InstallLayout::SideBySide => {
            install_binary_as(&bin, layout.main_name(), executable)?;
            for name in COMMAND_NAMES
                .iter()
                .copied()
                .filter(|name| *name != "crackle")
            {
                let destination = bin.join(name);
                if fs::symlink_metadata(&destination).is_ok() {
                    continue;
                }
                install_binary_as(&bin, name, executable)?;
            }
            if legacy_install_present(prefix) {
                println!(
                    "Legacy Crackle detected; installed side-by-side as {}. Existing Crackle files were left untouched.",
                    bin.join(layout.main_name()).display()
                );
            } else {
                println!(
                    "Installed Crackle side-by-side as {}.",
                    bin.join(layout.main_name()).display()
                );
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.crackle-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let mut file = fs::File::create(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    fs::rename(temporary, path)
}

fn setup() -> io::Result<()> {
    ensure_not_root()?;
    let layout = resolve_layout(&invoked_name())?;
    let run_full_setup =
        !layout.is_side_by_side() || env::var("CRACKLE_SETUP_NIX").ok().as_deref() == Some("1");
    let update_nix = if run_full_setup {
        is_installed_crackle(&env::current_exe()?, &prefix()?)
    } else {
        false
    };
    install_binaries(layout)?;
    if !run_full_setup {
        println!(
            "Side-by-side install complete. Nix and home-manager were not modified. Set CRACKLE_SETUP_NIX=1 to run the full setup."
        );
        return Ok(());
    }
    setup_environment(layout)?;
    if update_nix {
        nix_manager(&[OsString::from("update")])?;
    }
    Ok(())
}

fn is_installed_crackle(executable: &Path, prefix: &Path) -> bool {
    let bin = prefix.join("bin");
    executable
        .parent()
        .is_some_and(|parent| parent == bin.as_path())
}

/// Parsed `/etc/os-release`. The target device (Ubuntu Touch 24.04) reports
/// `ID=ubuntu`, `VARIANT=Touch`, `VARIANT_ID=touch`, `VERSION_CODENAME=noble`.
#[derive(Debug, Default, PartialEq, Eq)]
struct OsRelease {
    id: String,
    id_like: String,
    name: String,
    pretty_name: String,
    variant: String,
    variant_id: String,
    version_codename: String,
    ubuntu_codename: String,
}

impl OsRelease {
    fn parse(contents: &str) -> Self {
        let mut release = Self::default();
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            match key.trim() {
                "ID" => release.id = value.to_owned(),
                "ID_LIKE" => release.id_like = value.to_owned(),
                "NAME" => release.name = value.to_owned(),
                "PRETTY_NAME" => release.pretty_name = value.to_owned(),
                "VARIANT" => release.variant = value.to_owned(),
                "VARIANT_ID" => release.variant_id = value.to_owned(),
                "VERSION_CODENAME" => release.version_codename = value.to_owned(),
                "UBUNTU_CODENAME" => release.ubuntu_codename = value.to_owned(),
                _ => {}
            }
        }
        release
    }

    fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .map(|contents| Self::parse(&contents))
            .unwrap_or_default()
    }

    fn is_ubuntu(&self) -> bool {
        self.id.eq_ignore_ascii_case("ubuntu")
            || self.id.eq_ignore_ascii_case("ubuntu-touch")
            || self
                .id_like
                .split_whitespace()
                .any(|part| part.eq_ignore_ascii_case("ubuntu"))
            || self.name.to_lowercase().contains("ubuntu")
    }

    fn is_touch(&self) -> bool {
        self.variant.eq_ignore_ascii_case("touch")
            || self.variant_id.eq_ignore_ascii_case("touch")
            || self.id.eq_ignore_ascii_case("ubuntu-touch")
            || self.name.to_lowercase().contains("touch")
            || self.pretty_name.to_lowercase().contains("touch")
    }

    fn codename(&self) -> Option<&str> {
        if !self.version_codename.is_empty() {
            Some(&self.version_codename)
        } else if !self.ubuntu_codename.is_empty() {
            Some(&self.ubuntu_codename)
        } else {
            None
        }
    }

    fn display_id(&self) -> &str {
        if self.id.is_empty() {
            "this operating system"
        } else {
            &self.id
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AptSourceMode {
    /// Ubuntu <= 20.04: a single `/etc/apt/sources.list`.
    FocalList,
    /// Ubuntu >= 24.04: `/etc/apt/sources.list.d/*`.
    NobleDirectory,
    None,
}

fn apt_source_mode(codename: &str) -> AptSourceMode {
    if codename.eq_ignore_ascii_case("focal") {
        AptSourceMode::FocalList
    } else if codename.eq_ignore_ascii_case("noble") {
        AptSourceMode::NobleDirectory
    } else {
        AptSourceMode::None
    }
}

fn system_codename() -> String {
    if let Some(codename) = OsRelease::load(Path::new("/etc/os-release")).codename() {
        return codename.to_owned();
    }
    command_output("lsb_release", &["-cs"])
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default()
}

#[derive(Debug, PartialEq, Eq)]
enum SetupPlan {
    TouchUserSpaceApt,
    TouchNixReady,
    WritableApt,
}

fn setup_plan(
    is_ubuntu: bool,
    is_touch: bool,
    root_status: &str,
    nix_status: &str,
    nix_available: bool,
) -> io::Result<SetupPlan> {
    if is_ubuntu && is_touch {
        if nix_status != "rw" {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Ubuntu Touch requires a writable /nix. Crackle will not remount the read-only system image.",
            ));
        }
        if nix_available {
            Ok(SetupPlan::TouchNixReady)
        } else {
            Ok(SetupPlan::TouchUserSpaceApt)
        }
    } else if root_status == "rw" {
        Ok(SetupPlan::WritableApt)
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this operating system is currently not supported",
        ))
    }
}

fn setup_environment(layout: InstallLayout) -> io::Result<()> {
    let root_status = mount_option(Path::new("/"))?.unwrap_or("unknown");
    let nix_status = mount_option(Path::new("/nix"))?.unwrap_or("unknown");
    let release = OsRelease::load(Path::new("/etc/os-release"));
    let is_ubuntu = release.is_ubuntu();
    let is_touch = release.is_touch();
    let plan = setup_plan(
        is_ubuntu,
        is_touch,
        root_status,
        nix_status,
        available("nix"),
    )
    .map_err(|error| {
        if is_ubuntu && is_touch {
            error
        } else {
            io::Error::new(
                error.kind(),
                format!("{} is currently not supported", release.display_id()),
            )
        }
    })?;
    let mut apt = None;
    let mut used_user_space_apt = false;

    match plan {
        SetupPlan::TouchUserSpaceApt => {
            used_user_space_apt = true;
            apt = Some(AptWorkspace::create(layout)?);
            ensure_curl_xz(apt.as_mut().unwrap())?;
        }
        SetupPlan::TouchNixReady => {}
        SetupPlan::WritableApt => {
            if !available("apt") {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "only Debian-based systems are currently supported",
                ));
            }
            let mut missing = Vec::new();
            if !curl_available() {
                missing.push("curl");
            }
            if !available("xz") {
                missing.push("xz-utils");
            }
            if !missing.is_empty() {
                let mut args = vec!["apt", "install", "-y"];
                args.extend(missing);
                sudo_command(&args)?;
            }
        }
    }

    setup_nix_home_manager()?;
    drop(apt);
    if used_user_space_apt {
        cleanup_environment(layout);
        eprintln!(
            "Set up complete. Log in again, or run:\n  . {}/.nix-profile/etc/profile.d/nix.sh",
            home()?.display()
        );
    } else {
        eprintln!("Set up complete.");
    }
    Ok(())
}

struct AptWorkspace {
    base: PathBuf,
    cache: PathBuf,
    state: PathBuf,
    dpkg: PathBuf,
    config: PathBuf,
    data: PathBuf,
    temporary_links: Vec<PathBuf>,
    owned_paths: Vec<PathBuf>,
}

impl AptWorkspace {
    fn create(layout: InstallLayout) -> io::Result<Self> {
        // PIDs get reused; mix in a monotonic timestamp so two runs can
        // never collide on the same workspace directory name.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let unique = format!("crackle-apt-{}-{nanos}", std::process::id());
        let cache = apt_path(layout, "APTCACHE", "XDG_CACHE_HOME", ".cache", "apt")?;
        let state = apt_path(layout, "APTSTATE", "XDG_STATE_HOME", ".local/state", "apt")?;
        let dpkg = apt_path(
            layout,
            "DPKGSTATE",
            "XDG_STATE_HOME",
            ".local/state",
            "dpkg",
        )?;
        let config = apt_path(layout, "APTCONFIG", "XDG_CONFIG_HOME", ".config", "apt")?;
        let data = pkg_path(layout)?;
        let base = data.join(&unique);
        let mut owned_paths = Vec::new();
        let config_existed = config.exists();
        for path in [&cache, &state, &dpkg, &config, &data, &base] {
            if !path.exists() {
                fs::create_dir_all(path)?;
                cleanup::mark_owned(path);
                owned_paths.push(path.to_path_buf());
            } else {
                fs::create_dir_all(path)?;
            }
        }
        let status = Path::new("/var/lib/dpkg/status");
        if status.is_file() && !dpkg.join("status").exists() {
            fs::copy(status, dpkg.join("status"))?;
        }
        // Legacy `pkgsetup` only populates the APT config when it created it.
        if !config_existed {
            let sources = config.join("sources.list.d");
            let trusted = config.join("trusted.gpg.d");
            fs::create_dir_all(&sources)?;
            fs::create_dir_all(&trusted)?;
            match apt_source_mode(&system_codename()) {
                AptSourceMode::FocalList => {
                    let source = Path::new("/etc/apt/sources.list");
                    if source.is_file() {
                        copy_or_link(source, &config.join("sources.list"))?;
                    }
                }
                AptSourceMode::NobleDirectory => {
                    copy_directory_files(Path::new("/etc/apt/sources.list.d"), &sources, "")?;
                }
                AptSourceMode::None => {}
            }
            copy_directory_files(Path::new("/etc/apt/trusted.gpg.d"), &trusted, "")?;
        }
        Ok(Self {
            base,
            cache,
            state,
            dpkg,
            config,
            data,
            temporary_links: Vec::new(),
            owned_paths,
        })
    }

    fn apt_get(&self) -> Command {
        let mut command = command("apt-get");
        command.args([
            "-o",
            &format!("Dir::Cache={}", self.cache.display()),
            "-o",
            &format!("Dir::State={}", self.state.display()),
            "-o",
            &format!("Dir::State::status={}", self.dpkg.join("status").display()),
            "-o",
            &format!("Dir::Etc={}", self.config.display()),
        ]);
        command
    }

    fn apt_cache(&self) -> Command {
        let mut command = command("apt-cache");
        command.args([
            "-o",
            &format!("Dir::Cache={}", self.cache.display()),
            "-o",
            &format!("Dir::State={}", self.state.display()),
            "-o",
            &format!("Dir::Etc={}", self.config.display()),
        ]);
        command
    }
}

impl Drop for AptWorkspace {
    fn drop(&mut self) {
        for link in &self.temporary_links {
            if let Err(error) = fs::remove_file(link)
                && error.kind() != io::ErrorKind::NotFound
            {
                eprintln!("Crackle: could not remove temporary Apt tool link: {error}");
            }
        }
        if let Err(error) = fs::remove_dir_all(&self.base)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!("Crackle: could not clean temporary Apt workspace: {error}");
        }
        for path in self.owned_paths.iter().rev() {
            if let Err(error) = fs::remove_dir_all(path)
                && error.kind() != io::ErrorKind::NotFound
            {
                eprintln!(
                    "Crackle: could not clean temporary Apt path {}: {error}",
                    path.display()
                );
            }
        }
    }
}

fn copy_or_link(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, destination)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::copy(source, destination)?;
        Ok(())
    }
}

fn copy_directory_files(source: &Path, destination: &Path, suffix: &str) -> io::Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && (suffix.is_empty()
                || path
                    .extension()
                    .is_some_and(|extension| format!(".{}", extension.to_string_lossy()) == suffix))
        {
            let target = destination.join(entry.file_name());
            if !target.exists() {
                copy_or_link(&path, &target)?;
            }
        }
    }
    Ok(())
}

/// `.deb` files present in the Apt archive cache. A missing archive directory
/// is normal when nothing had to be downloaded (for example curl/xz are already
/// installed); it must not abort setup.
fn downloaded_debs(archives: &Path) -> io::Result<Vec<PathBuf>> {
    if !archives.is_dir() {
        return Ok(Vec::new());
    }
    let mut debs = Vec::new();
    for entry in fs::read_dir(archives).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not read {}: {error}", archives.display()),
        )
    })? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "deb") {
            debs.push(path);
        }
    }
    Ok(debs)
}

fn ensure_curl_xz(apt: &mut AptWorkspace) -> io::Result<()> {
    let mut missing = Vec::new();
    if !curl_available() {
        missing.push("curl");
    }
    if !available("xz") {
        missing.push("xz-utils");
    }
    // Legacy always refreshed lists here; only the empty `apt-get install`
    // invocation (a legacy bug) is skipped when both tools already exist.
    let status = run_command(apt.apt_get().arg("update"))?;
    if !status.success() {
        return Err(io::Error::other("apt-get update failed"));
    }
    if !missing.is_empty() {
        let mut command = apt.apt_get();
        command.args(["install", "--download-only", "-y"]);
        command.args(&missing);
        let status = run_command(&mut command)?;
        if !status.success() {
            return Err(io::Error::other("apt-get download-only install failed"));
        }
    }
    let bin_dir = prefix()?.join("bin");
    fs::create_dir_all(&bin_dir)?;
    let archives = apt.cache.join("archives");
    let package_dir = apt.data.join("packages");
    fs::create_dir_all(&package_dir)?;
    for path in downloaded_debs(&archives)? {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let package_name = file_name.split('_').next().unwrap_or(file_name);
        if package_name.is_empty() {
            continue;
        }
        let output = package_dir.join(package_name);
        run_os(
            "dpkg-deb",
            &[
                OsString::from("-x"),
                path.into_os_string(),
                output.clone().into_os_string(),
            ],
        )?;
        let extracted_bin = output.join("usr/bin");
        let Ok(entries) = fs::read_dir(&extracted_bin) else {
            continue;
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let destination = bin_dir.join(entry.file_name());
            if fs::symlink_metadata(&destination).is_ok() {
                continue;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(entry.path(), &destination)?;
            #[cfg(not(unix))]
            fs::copy(entry.path(), &destination)?;
            apt.temporary_links.push(destination);
        }
    }
    Ok(())
}

fn setup_nix_home_manager() -> io::Result<()> {
    if !available("nix") {
        println!("Installing Nix...");
        let installer = env::temp_dir().join(format!("crackle-nix-install-{}", std::process::id()));
        download_file("https://nixos.org/nix/install", &installer).map_err(|error| {
            io::Error::other(format!("failed to download the Nix installer: {error}"))
        })?;
        let status = run_command(
            command("sh")
                .arg(&installer)
                .args(["--no-daemon", "--no-channel-add"])
                .env("PATH", path_with_working_curl()),
        )?;
        let _ = fs::remove_file(installer);
        if !status.success() {
            return Err(io::Error::other("Nix installer failed"));
        }
        // Legacy sourced nix.sh (best effort) and then always configured the
        // nixpkgs channel, even when the profile file could not be sourced.
        let channel = format!("https://nixos.org/channels/nixos-{NIX_VERSION}");
        let profile = home()?.join(".nix-profile/etc/profile.d/nix.sh");
        if profile.is_file() {
            let output = run_command(
                command("sh")
                    .args([
                        "-c",
                        ". \"$1\"; nix-channel --add \"$2\" nixpkgs; nix-channel --update",
                        "crackle",
                    ])
                    .arg(profile)
                    .arg(&channel),
            )?;
            if !output.success() {
                return Err(io::Error::other("failed to configure the Nix channel"));
            }
        } else {
            run("nix-channel", &["--add", &channel, "nixpkgs"])?;
            run("nix-channel", &["--update"])?;
        }
    }
    if !available("home-manager") {
        run(
            "nix-channel",
            &[
                "--add",
                &format!(
                    "https://github.com/nix-community/home-manager/archive/release-{NIX_VERSION}.tar.gz"
                ),
                "home-manager",
            ],
        )?;
        run("nix-channel", &["--update"])?;
        run("nix-shell", &["<home-manager>", "-A", "install"])?;
    }
    Ok(())
}

fn cleanup_workspace(layout: InstallLayout) -> io::Result<cleanup::Cleanup> {
    let prefix = prefix()?;
    Ok(cleanup::workspace(
        prefix.clone(),
        pkg_path(layout)?,
        apt_path(layout, "APTCACHE", "XDG_CACHE_HOME", ".cache", "apt")?,
        apt_path(layout, "APTSTATE", "XDG_STATE_HOME", ".local/state", "apt")?,
        apt_path(layout, "APTCONFIG", "XDG_CONFIG_HOME", ".config", "apt")?,
        apt_path(
            layout,
            "DPKGSTATE",
            "XDG_STATE_HOME",
            ".local/state",
            "dpkg",
        )?,
        xdg_path("XDG_STATE_HOME", ".local/state")?,
        library_dir(layout, &prefix),
        COMMAND_NAMES,
    ))
}

fn cleanup_environment(layout: InstallLayout) {
    match cleanup_workspace(layout) {
        Ok(cleanup) => cleanup.run(),
        Err(error) => eprintln!("Crackle: could not resolve cleanup paths: {error}"),
    }
}

/// `crackle clean`: legacy ran `nix-manager clean` and then `pkgunset`, with no
/// `set -e`, so a failing garbage collection never blocked cleanup.
fn clean() -> io::Result<()> {
    let layout = resolve_layout(&invoked_name())?;
    if let Err(error) = nix_manager(&[OsString::from("clean")]) {
        eprintln!("Crackle: nix clean failed: {error}");
    }
    cleanup_environment(layout);
    Ok(())
}

/// `crackle update` owns the channel-add steps; `nix-manager update` only
/// updates channels and switches, matching the legacy split.
fn update() -> io::Result<()> {
    run(
        "nix-channel",
        &[
            "--add",
            &format!("https://nixos.org/channels/nixos-{NIX_VERSION}"),
            "nixpkgs",
        ],
    )?;
    run(
        "nix-channel",
        &[
            "--add",
            &format!(
                "https://github.com/nix-community/home-manager/archive/release-{NIX_VERSION}.tar.gz"
            ),
            "home-manager",
        ],
    )?;
    nix_manager(&[OsString::from("update")])
}

/// Legacy `_backup_config_file`: leave an identical backup untouched.
fn backup_config(path: &Path) -> io::Result<PathBuf> {
    let backup = PathBuf::from(format!("{}.backup", path.display()));
    let current = fs::read(path)?;
    if fs::read(&backup).is_ok_and(|existing| existing == current) {
        return Ok(backup);
    }
    fs::write(&backup, current)?;
    Ok(backup)
}

/// Legacy `apply_config` captured both streams and surfaced the errors when the
/// build failed. Success output is discarded, as before.
fn build_configuration() -> io::Result<()> {
    let output = command("home-manager")
        .args(["build", "--no-out-link"])
        .output()?;
    check_interrupted()?;
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.trim().is_empty() {
        println!("{}", stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        println!();
        println!("{}", stderr.trim_end());
    }
    Err(io::Error::other("home-manager build failed"))
}

fn nix_manager(args: &[OsString]) -> io::Result<()> {
    let operation = args
        .first()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "argument missing"))?;
    let packages: Vec<String> = args[1..]
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    match operation {
        "list" => {
            let path = config::config_path()?;
            for package in config::read_packages(&path, "home")? {
                println!("{}", package.strip_prefix("pkgs.").unwrap_or(&package));
            }
        }
        "install" | "remove" => {
            if packages.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "argument missing: provide one or more package names",
                ));
            }
            let path = config::config_path()?;
            let names = packages.join(", ");
            let backup = backup_config(&path)?;
            let packages: Vec<String> = packages
                .into_iter()
                .map(|package| {
                    if package.starts_with("pkgs.") {
                        package
                    } else {
                        format!("pkgs.{package}")
                    }
                })
                .collect();
            let (additions, removals): (Vec<String>, Vec<String>) = if operation == "install" {
                (packages, Vec::new())
            } else {
                (Vec::new(), packages)
            };
            if operation == "install" {
                println!("Installing {names}...");
            } else {
                println!("Removing {names}...");
            }
            config::update_packages(&path, "home", &additions, &removals, false)?;
            if let Err(error) = build_configuration() {
                restore_config(&path, &backup)?;
                return Err(error);
            }
            if operation == "install" {
                println!("Activating...");
            } else {
                println!("Deactivating...");
            }
            if let Err(error) = run("home-manager", &["switch"]) {
                restore_config(&path, &backup)?;
                return match run("home-manager", &["switch"]) {
                    Ok(()) => Err(error),
                    Err(recovery_error) => Err(io::Error::other(format!(
                        "{error}; failed to reactivate the backed-up configuration: {recovery_error}"
                    ))),
                };
            }
            if operation == "remove" {
                println!("Cleaning up...");
                run("nix-collect-garbage", &["-d"])?;
            }
        }
        "clean" => {
            println!("Cleaning up...");
            run("nix-collect-garbage", &["-d"])?;
            run("nix-store", &["--optimise"])?;
        }
        "update" => {
            println!("Updating...");
            run("nix-channel", &["--update"])?;
            println!("Activating...");
            run("home-manager", &["switch"])?;
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown nix-manager command '{operation}'"),
            ));
        }
    }
    Ok(())
}

fn restore_config(path: &Path, backup: &Path) -> io::Result<()> {
    let temporary = path.with_extension("nix.crackle-restore");
    fs::copy(backup, &temporary)?;
    fs::rename(temporary, path)
}

fn apt_workspace(layout: InstallLayout) -> io::Result<AptWorkspace> {
    AptWorkspace::create(layout)
}

fn setup_repositories(apt: &AptWorkspace, layout: InstallLayout) -> io::Result<()> {
    let archive = apt.base.join("cracklebin.tar.gz");
    download_file(CRACKLE_BIN_ARCHIVE, &archive).map_err(|error| {
        io::Error::other(format!(
            "failed to download Crackle's Apt repository configuration: {error}"
        ))
    })?;

    let listing = capture_command(command("tar").args(["-tzf"]).arg(&archive))?;
    if !listing.status.success() {
        return Err(io::Error::other(
            "could not read Crackle's Apt repository archive",
        ));
    }
    let members = String::from_utf8_lossy(&listing.stdout);
    let preferences = apt.config.join("preferences.d");
    let sources = apt.config.join("sources.list.d");
    let library = library_dir(layout, &prefix()?);
    fs::create_dir_all(&preferences)?;
    fs::create_dir_all(&sources)?;
    fs::create_dir_all(&library)?;
    for member in members.lines().filter(|member| !member.ends_with('/')) {
        let Some(relative) = member.strip_prefix("cracklebin-master/") else {
            continue;
        };
        let relative_path = Path::new(relative);
        if relative_path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsafe path in Crackle's Apt repository archive",
            ));
        }
        let destination = if relative.starts_with("etc/preferences.d/") {
            preferences.join(relative_path.file_name().unwrap())
        } else if relative.starts_with("etc/sources.list.d/") {
            sources.join(relative_path.file_name().unwrap())
        } else {
            library.join(relative_path)
        };
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let extracted = capture_command(
            command("tar")
                .args(["-xOzf"])
                .arg(&archive)
                .arg("--")
                .arg(member),
        )?;
        if !extracted.status.success() {
            return Err(io::Error::other(format!(
                "failed to extract repository file '{member}'"
            )));
        }
        atomic_write(&destination, &extracted.stdout)?;
    }

    const REPOSITORY_KEYS: [(&str, &str, &str); 3] = [
        (
            "microsoft",
            "https://packages.microsoft.com/keys/microsoft.asc",
            "microsoft.asc",
        ),
        (
            "mozilla",
            "https://packages.mozilla.org/apt/repo-signing-key.gpg",
            "packages.mozilla.org.asc",
        ),
        (
            "tailscale",
            "https://pkgs.tailscale.com/stable/ubuntu/focal.noarmor.gpg",
            "tailscale-archive-keyring.gpg",
        ),
    ];
    let trusted = apt.config.join("trusted.gpg.d");
    fs::create_dir_all(&trusted)?;
    for entry in fs::read_dir(&sources)? {
        let source = entry?.path();
        if source
            .extension()
            .is_none_or(|extension| extension != "list")
        {
            continue;
        }
        let name = source
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if let Some((_, url, key_name)) = REPOSITORY_KEYS.iter().find(|(repo, _, _)| *repo == name)
        {
            let key = trusted.join(key_name);
            download_file(url, &key).map_err(|error| {
                io::Error::other(format!(
                    "failed to download the {name} Apt signing key: {error}"
                ))
            })?;
        } else {
            // Legacy sourced every downloaded repo script; the Rust port keeps
            // a declarative key table instead of executing fetched code.
            eprintln!(
                "Crackle: no signing key is configured for Apt source '{name}'; leaving it untouched"
            );
        }
        let contents = fs::read_to_string(&source)?;
        atomic_write(
            &source,
            resolve_apt_source_paths(&contents, &apt.config, &home()?).as_bytes(),
        )?;
    }
    Ok(())
}

fn clean_apt(apt: &AptWorkspace) -> io::Result<()> {
    let status = run_command(apt.apt_get().arg("clean"))?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("apt-get clean failed"))
    }
}

fn resolve_apt_source_paths(contents: &str, apt_config: &Path, home: &Path) -> String {
    let apt_config = apt_config.to_string_lossy();
    let home = home.to_string_lossy();
    contents
        .replace("$HOME/.config/apt", &apt_config)
        .replace("$HOME", &home)
}

fn check_connectivity() -> io::Result<()> {
    let status = run_command(command("wget").args([
        "--quiet",
        "--spider",
        "https://connectivity-check.ubuntu-touch.io/nm.txt",
    ]))?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "no internet connection; run `crackle update` later to fetch the latest package lists",
        ))
    }
}

fn crack(args: &[OsString]) -> io::Result<()> {
    ensure_not_root()?;
    if args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing argument",
        ));
    }
    let layout = resolve_layout(&invoked_name())?;
    let mut apt = apt_workspace(layout)?;
    ensure_curl_xz(&mut apt)?;
    clean_apt(&apt)?;
    setup_repositories(&apt, layout)?;
    let mut update = apt.apt_get();
    update.arg("update");
    if !run_command(&mut update)?.success() {
        return Err(io::Error::other("apt-get update failed"));
    }
    let mut download = apt.apt_get();
    download.args(["install", "--download-only"]);
    download.args(args);
    if !run_command(&mut download)?.success() {
        return Err(io::Error::other("apt-get download-only install failed"));
    }
    let package_name = args
        .first()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    let destination = home()?.join("packages").join(package_name.as_ref());
    fs::create_dir_all(&destination)?;
    for path in downloaded_debs(&apt.cache.join("archives"))? {
        println!("extracting {}", path.display());
        run_os(
            "dpkg-deb",
            &[
                OsString::from("-x"),
                path.into_os_string(),
                destination.clone().into_os_string(),
            ],
        )?;
    }
    println!("Extracted packages to {}.", destination.display());
    cleanup_environment(layout);
    Ok(())
}

fn click(args: &[OsString], bump: bool) -> io::Result<()> {
    if args.first().is_some_and(|value| value == "debug") {
        return click_debug();
    }
    ensure_not_root()?;
    if args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing argument",
        ));
    }
    if bump {
        let root = env::var_os("ROOT")
            .map(PathBuf::from)
            .unwrap_or(env::current_dir()?);
        let manifest = root.join("manifest.json");
        return bump_version(args, &manifest);
    }
    if !available("click") {
        return Err(io::Error::new(io::ErrorKind::NotFound, "missing click"));
    }
    let root = env::var_os("ROOT")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?);
    let manifest = root.join("manifest.json");
    if !manifest.is_file() && !root.join("manifest.json.in").is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "missing manifest.json",
        ));
    }
    let arch = click_arch();
    let triplet = click_triplet();
    let build = env::var_os("BUILD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("build"));
    let install = env::var_os("INSTALL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("install"));
    let library_dir = env::var_os("CLICK_LD_LIBRARY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| install.join("lib").join(&triplet));
    let click_bin = env::var_os("CLICK_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| library_dir.join("bin"));
    if build.exists() {
        fs::remove_dir_all(&build)?;
    }
    fs::create_dir_all(&build)?;
    let layout = resolve_layout(&invoked_name())?;
    let mut apt = apt_workspace(layout)?;
    check_connectivity()?;
    ensure_curl_xz(&mut apt)?;
    clean_apt(&apt)?;
    setup_repositories(&apt, layout)?;
    let mut update = apt.apt_get();
    update.arg("update");
    if !run_command(&mut update)?.success() {
        return Err(io::Error::other("apt-get update failed"));
    }
    let mut packages = args.to_vec();
    for package in &mut packages {
        *package = package_with_arch(package, &arch);
    }
    let mut download = apt.apt_get();
    download.args(["install", "--download-only", "-y"]);
    download.args(&packages);
    if !run_command(&mut download)?.success() {
        return Err(io::Error::other("apt-get download-only install failed"));
    }
    for deb in downloaded_debs(&apt.cache.join("archives"))? {
        println!("extracting {}", deb.display());
        run_os(
            "dpkg-deb",
            &[
                OsString::from("-x"),
                deb.into_os_string(),
                build.clone().into_os_string(),
            ],
        )?;
    }
    prepare_click_manifest(&root, &build, &install, &click_bin, args)?;
    println!("Building package tree...");
    fs::create_dir_all(&library_dir)?;
    fs::create_dir_all(&click_bin)?;
    for source in [build.join("lib"), build.join("usr/lib")] {
        copy_tree_if_exists(&source, &library_dir)?;
    }
    let triplet_dir = library_dir.join(&triplet);
    if triplet_dir.is_dir() {
        copy_tree_if_exists(&triplet_dir, &library_dir)?;
        fs::remove_dir_all(triplet_dir)?;
    }
    for source in [
        build.join("usr/bin"),
        build.join("usr/sbin"),
        build.join("usr/games"),
    ] {
        copy_tree_if_exists(&source, &click_bin)?;
    }
    copy_tree_if_exists(&build.join("usr/share"), &install.join("share"))?;
    let adapt = build.join("adapt.sh");
    if manifest.is_file() && adapt.is_file() {
        // Legacy passes `$PKG`, a Bash array, which expands to its first element.
        let package_name = args
            .first()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let status = run_command(
            command("bash")
                .arg(adapt)
                .current_dir(&root)
                .env("ROOT", &root)
                .env("ARCH", &arch)
                .env("ARCH_TRIPLET", &triplet)
                .env("BUILD_DIR", &build)
                .env("INSTALL_DIR", &install)
                .env("CLICK_LD_LIBRARY_PATH", &library_dir)
                .env("CLICK_PATH", &click_bin)
                .env("PKG", package_name)
                .env("SCRP_DIR", prefix()?.join("bin")),
        )?;
        if !status.success() {
            return Err(io::Error::other("Click adapt.sh failed"));
        }
        let manifest_path = install.join("manifest.json");
        if manifest_path.is_file() {
            let contents = fs::read_to_string(&manifest_path)?.replace("@CLICK_ARCH@", &arch);
            atomic_write(&manifest_path, contents.as_bytes())?;
        }
    }
    if env::var_os("CI").is_none_or(|value| value.is_empty()) {
        println!("Building click...");
        run_os(
            "click",
            &[OsString::from("build"), install.into_os_string()],
        )?;
    }
    cleanup_environment(layout);
    Ok(())
}

fn click_debug() -> io::Result<()> {
    let root = env::var_os("ROOT")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?);
    let install = env::var_os("INSTALL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("install"));
    let triplet = click_triplet();
    let library_dir = env::var_os("CLICK_LD_LIBRARY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| install.join("lib").join(&triplet));
    let click_bin = env::var_os("CLICK_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| library_dir.join("bin"));
    println!("CLICK CONFIG");
    println!("ROOT: '{}'", root.display());
    println!("ARCH: '{}'", click_arch());
    println!("ARCH_TRIPLET: '{triplet}'");
    println!(
        "BUILD_DIR: '{}'",
        env::var_os("BUILD_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("build"))
            .display()
    );
    println!("INSTALL_DIR: '{}'", install.display());
    println!("CLICK_LD_LIBRARY_PATH: '{}'", library_dir.display());
    println!("CLICK_PATH: '{}'", click_bin.display());
    Ok(())
}

fn click_arch() -> String {
    env::var("ARCH").unwrap_or_else(|_| {
        command_output("dpkg", &["--print-architecture"])
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                if env::consts::ARCH == "aarch64" {
                    "arm64".to_string()
                } else {
                    "amd64".to_string()
                }
            })
    })
}

fn click_triplet() -> String {
    env::var("ARCH_TRIPLET").unwrap_or_else(|_| {
        command_output("dpkg-architecture", &["-qDEB_HOST_MULTIARCH"])
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fallback_triplet().to_string())
    })
}

fn package_with_arch(package: &OsString, arch: &str) -> OsString {
    if package.to_string_lossy().ends_with(":all") {
        package.clone()
    } else {
        OsString::from(format!("{}:{arch}", package.to_string_lossy()))
    }
}

fn prepare_click_manifest(
    root: &Path,
    build: &Path,
    install: &Path,
    click_bin: &Path,
    packages: &[OsString],
) -> io::Result<()> {
    let manifest = root.join("manifest.json");
    let template = root.join("manifest.json.in");
    if !manifest.is_file() && !template.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "manifest.json or manifest.json.in is missing",
        ));
    }
    // Legacy `build_click` only calls `prep_build` when there is no template;
    // with `manifest.json.in`, `adapt.sh` owns the install tree.
    if template.is_file() {
        return Ok(());
    }

    if install.exists() {
        fs::remove_dir_all(install)?;
    }
    fs::create_dir_all(install)?;
    println!("Preparing build environment...");
    copy_tree_if_exists(&root.join("pkg.d"), build)?;
    for required in ["apparmor.json", "manifest.json"] {
        let source = root.join(required);
        if !source.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{required} is missing"),
            ));
        }
        fs::copy(source, build.join(required))?;
    }
    let name = packages
        .first()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let manifest_text = fs::read_to_string(build.join("manifest.json"))?;
    atomic_write(
        &build.join("manifest.json"),
        manifest_text.replace("@pkg@", &name).as_bytes(),
    )?;
    for entry in fs::read_dir(build)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            fs::copy(&path, install.join(path.file_name().unwrap()))?;
        }
    }
    let package_script = build.join("pkg.sh");
    if package_script.is_file() {
        fs::create_dir_all(click_bin)?;
        fs::copy(package_script, click_bin.join(format!("{name}.sh")))?;
    }
    Ok(())
}

fn copy_tree_if_exists(source: &Path, destination: &Path) -> io::Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            fs::create_dir_all(&target)?;
            copy_tree_if_exists(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Legacy `bump_version`: `${candidate%~*}`, `${...%+*}`, `${...#*:}`.
fn parse_candidate_version(candidate: &str) -> String {
    let stripped = candidate.split(['~', '+']).next().unwrap_or(candidate);
    stripped
        .split_once(':')
        .map(|(_, version)| version)
        .unwrap_or(stripped)
        .to_owned()
}

/// Legacy awk read field 2 of the first line containing `version`, removed
/// quotes, then dropped the trailing comma (`"version": "1.2.3",` → `1.2.3`).
fn manifest_version(contents: &str) -> Option<String> {
    for line in contents.lines() {
        if !line.contains("version") {
            continue;
        }
        let mut fields = line.split_whitespace();
        if fields.next().is_none() {
            continue;
        }
        let Some(raw) = fields.next() else {
            continue;
        };
        let value = raw.trim_end_matches(',').trim_matches('"');
        if !value.is_empty() {
            return Some(value.to_owned());
        }
    }
    None
}

/// Replace the value of the first quoted `version` value, never the JSON key.
fn replace_manifest_version(contents: &str, current: &str, new_version: &str) -> Option<String> {
    let mut output = String::with_capacity(contents.len());
    let mut replaced = false;
    for line in contents.split_inclusive('\n') {
        if !replaced && line.contains("version") {
            let quotes: Vec<usize> = line
                .match_indices('"')
                .map(|(position, _)| position)
                .collect();
            if quotes.len() >= 4 {
                let start = quotes[2] + 1;
                let end = quotes[3];
                output.push_str(&line[..start]);
                output.push_str(new_version);
                output.push_str(&line[end..]);
                replaced = true;
                continue;
            }
        }
        output.push_str(line);
    }
    if replaced {
        Some(output)
    } else if contents.contains(current) {
        Some(contents.replacen(current, new_version, 1))
    } else {
        None
    }
}

fn bump_version(args: &[OsString], manifest: &Path) -> io::Result<()> {
    let layout = resolve_layout(&invoked_name())?;
    let mut apt = apt_workspace(layout)?;
    check_connectivity()?;
    ensure_curl_xz(&mut apt)?;
    clean_apt(&apt)?;
    setup_repositories(&apt, layout)?;
    let mut update = apt.apt_get();
    update.arg("update");
    if !run_command(&mut update)?.success() {
        return Err(io::Error::other("apt-get update failed"));
    }
    let package = args
        .first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing argument"))?;
    let mut policy = apt.apt_cache();
    policy.arg("policy");
    policy.arg(package_with_arch(package, &click_arch()));
    let output = capture_command(&mut policy)?;
    if !output.status.success() {
        return Err(io::Error::other("apt-cache policy failed"));
    }
    let output_text = String::from_utf8_lossy(&output.stdout);
    let candidate = output_text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Candidate: "))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "package candidate not found"))?;
    let version = parse_candidate_version(candidate);
    let text = fs::read_to_string(manifest)?;
    let current = manifest_version(&text)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "manifest version not found"))?;
    println!("available version: {version}");
    println!("current version: {current}");
    if current != version {
        let updated = replace_manifest_version(&text, &current, &version).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "manifest version not found")
        })?;
        atomic_write(manifest, updated.as_bytes())?;
        println!("Updated manifest.json.");
    }
    Ok(())
}

fn sudo_command(args: &[&str]) -> io::Result<()> {
    println!("this action requires sudo access");
    run("sudo", args)
}

const SUDO_DEFAULT_BIN: &str = "/root/.local/bin";

fn sudo_link(args: &[OsString], enable: bool) -> io::Result<()> {
    if args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing argument",
        ));
    }
    if enable {
        enable_sudo(args)
    } else {
        disable_sudo(args)
    }
}

// Mirrors the original `enable_sudo()` in lib/sudofunc: config_sudo() when
// there's no Snap, unconfig_sudo() (which tears down the old setup and
// switches to /snap/bin) when there is, then one `ln -s` per package -
// only when the link doesn't already exist, never overwriting it.
fn enable_sudo(args: &[OsString]) -> io::Result<()> {
    let destination = if Path::new("/snap/bin").is_dir() {
        unconfigure_sudo_profile()?;
        PathBuf::from("/snap/bin")
    } else {
        configure_sudo_profile()?;
        PathBuf::from(SUDO_DEFAULT_BIN)
    };
    for name in args {
        let name = name.to_string_lossy();
        let resolved = capture_command(Command::new("which").arg(name.as_ref()))?;
        if !resolved.status.success() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("command '{name}' was not found"),
            ));
        }
        let path = PathBuf::from(String::from_utf8_lossy(&resolved.stdout).trim());
        let canonical = fs::canonicalize(path)?;
        let link = destination.join(name.as_ref());
        if !link.is_file() {
            sudo_command(&[
                "ln",
                "-s",
                canonical.to_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 executable path")
                })?,
                link.to_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 link path")
                })?,
            ])?;
        }
    }
    run("sudo", &["-k"])
}

// Mirrors the original `disable_sudo()`: it never looks at /snap/bin or
// SUDO_BIN directly - it resolves each package through `sudo -i which`
// (which sees root's PATH, including whatever enable_sudo wired up) and
// removes exactly what that resolves to.
fn disable_sudo(args: &[OsString]) -> io::Result<()> {
    for name in args {
        let name = name.to_string_lossy();
        println!("this action requires sudo access");
        let resolved = capture_command(command("sudo").args(["-i", "which", name.as_ref()]))?;
        if resolved.status.success() {
            let path = String::from_utf8_lossy(&resolved.stdout).trim().to_owned();
            if !path.is_empty() && Path::new(&path).is_file() {
                sudo_command(&["rm", path.as_str()])?;
            }
        }
    }
    run("sudo", &["-k"])
}

// Mirrors `config_sudo()`: ensure /root/.profile exists, add a PATH line
// if nothing mentioning "bin" is in it yet, and create SUDO_BIN if missing.
fn configure_sudo_profile() -> io::Result<()> {
    let profile = "/root/.profile";
    if !Path::new(profile).is_file() {
        sudo_command(&["touch", profile])?;
    }
    if !profile_mentions_bin(profile)? {
        let mut child = command("sudo")
            .args(["tee", "--append", profile])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("could not open sudo tee input"))?
            .write_all(format!("PATH={SUDO_DEFAULT_BIN}:$PATH\n").as_bytes())?;
        let status = child.wait()?;
        if !status.success() {
            return Err(io::Error::other("could not update /root/.profile"));
        }
    }
    if !Path::new(SUDO_DEFAULT_BIN).is_dir() {
        sudo_command(&["mkdir", "-p", SUDO_DEFAULT_BIN])?;
    }
    Ok(())
}

// Mirrors `unconfig_sudo()`: drop /root/.profile if it mentions "bin",
// remove the old SUDO_BIN directory entirely, then hand back control to
// /snap/bin.
fn unconfigure_sudo_profile() -> io::Result<()> {
    let profile = "/root/.profile";
    if profile_mentions_bin(profile)? {
        sudo_command(&["rm", profile])?;
    }
    if Path::new(SUDO_DEFAULT_BIN).is_dir() {
        sudo_command(&["rm", "-r", SUDO_DEFAULT_BIN])?;
    }
    Ok(())
}

fn profile_mentions_bin(profile: &str) -> io::Result<bool> {
    Ok(fs::read_to_string(profile).is_ok_and(|contents| contents.contains("bin")))
}

fn remove_path(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    if let Err(error) = result
        && error.kind() != io::ErrorKind::NotFound
    {
        eprintln!(
            "Crackle: could not remove {} during nuke: {error}",
            path.display()
        );
    }
}

fn same_contents(a: &Path, b: &Path) -> bool {
    match (fs::read(a), fs::read(b)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn remove_side_by_side_state(layout: InstallLayout, prefix: &Path) -> io::Result<()> {
    remove_path(&library_dir(layout, prefix));
    remove_path(&pkg_path(layout)?);
    for path in [
        apt_path(layout, "APTCACHE", "XDG_CACHE_HOME", ".cache", "apt")?,
        apt_path(layout, "APTSTATE", "XDG_STATE_HOME", ".local/state", "apt")?,
        apt_path(layout, "APTCONFIG", "XDG_CONFIG_HOME", ".config", "apt")?,
        apt_path(
            layout,
            "DPKGSTATE",
            "XDG_STATE_HOME",
            ".local/state",
            "dpkg",
        )?,
    ] {
        remove_path(&path);
    }
    remove_path(&state_dir(layout)?);
    Ok(())
}

fn confirm_nuke() -> io::Result<()> {
    use std::io::IsTerminal;
    if env::var("CRACKLE_YES").ok().as_deref() == Some("1") {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        // Non-interactive (scripts, CI): behave exactly like before, no prompt.
        return Ok(());
    }
    print!("This removes Crackle, Nix, and all home-manager state. Continue? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if answer.trim().eq_ignore_ascii_case("y") {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Interrupted, "nuke cancelled"))
    }
}

fn nuke() -> io::Result<()> {
    ensure_not_root()?;
    let layout = resolve_layout(&invoked_name())?;
    let prefix = prefix()?;
    let legacy = legacy_install_present(&prefix);
    let forced = env::var("CRACKLE_FORCE").ok().as_deref() == Some("1");
    nuke_allowed(layout, legacy, forced)?;
    confirm_nuke()?;

    println!("Removing Crackle, Nix, and home-manager state.");
    for channel in ["home-manager", "nixpkgs"] {
        if let Err(error) = run("nix-channel", &["--remove", channel]) {
            eprintln!("Crackle: could not remove Nix channel '{channel}': {error}");
        }
    }
    let root_is_rw = mount_option(Path::new("/")).ok().flatten() == Some("rw");
    let cleanup = if root_is_rw {
        run("sudo", &["rm", "-rf", "/nix"])
    } else {
        run(
            "sudo",
            &[
                "find",
                "/nix",
                "-mindepth",
                "1",
                "-maxdepth",
                "1",
                "-exec",
                "rm",
                "-rf",
                "{}",
                "+",
            ],
        )
    };
    if let Err(error) = cleanup {
        eprintln!("Crackle: could not remove /nix: {error}");
    }
    let home = home()?;
    for relative in [
        ".config/home-manager",
        ".local/share/home-manager",
        ".nix-profile",
        ".nix-defexpr",
        ".nix-channels",
        ".nix-command-errors.log",
        ".cache/nix",
        ".local/share/nix",
        ".local/state/nix",
    ] {
        remove_path(&home.join(relative));
    }

    if layout.is_side_by_side() && !legacy {
        // crackle-rust-only machine: remove only files this layout created.
        remove_path(&prefix.join("bin").join(layout.main_name()));
        if let Ok(executable) = env::current_exe() {
            for name in COMMAND_NAMES
                .iter()
                .copied()
                .filter(|name| *name != "crackle")
            {
                let path = prefix.join("bin").join(name);
                if same_contents(&path, &executable) {
                    remove_path(&path);
                }
            }
        }
    } else {
        // Standard install, or an explicit forced nuke: remove legacy names too.
        for relative in [
            "bin/crackle",
            "bin/crackle-rust",
            "bin/arch_triplet",
            "bin/desktop_trunc",
            "bin/_crackleopts",
            "bin/remount",
            "bin/rorw",
            "bin/nix-manager",
            "bin/osvariant",
            "bin/rwnix",
            "lib/basefunc",
            "lib/clickfunc",
            "lib/setupfunc",
            "lib/upgradefunc",
            "lib/sudofunc",
        ] {
            remove_path(&prefix.join(relative));
        }
        if let Ok(completion) = completion_file() {
            remove_path(&completion);
            let _ = fs::remove_dir(completion_dir()?);
        }
        let nix_config = prefix.join("lib/nix_config");
        if nix_config.is_dir() {
            for name in [
                "__init__.py",
                "config.py",
                "nix_config.py",
                "nix_interact.py",
                "openprocess.py",
            ] {
                remove_path(&nix_config.join(name));
            }
            remove_path(&nix_config.join("__pycache__"));
            let _ = fs::remove_dir(nix_config);
        }
        remove_path(&prefix.join("lib/crackle"));
        remove_path(&prefix.join("lib/crackle-rust"));
    }

    if layout.is_side_by_side() {
        remove_side_by_side_state(layout, &prefix)?;
    }
    Ok(())
}

fn migrate() -> io::Result<()> {
    ensure_not_root()?;
    migration::migrate(
        &prefix()?,
        &xdg_path("XDG_DATA_HOME", ".local/share")?,
        &state_dir(InstallLayout::Standard)?,
    )
}

fn rollback(id: Option<&str>) -> io::Result<()> {
    ensure_not_root()?;
    migration::rollback(
        &prefix()?,
        &xdg_path("XDG_DATA_HOME", ".local/share")?,
        &state_dir(InstallLayout::Standard)?,
        id,
    )
}

#[cfg(unix)]
extern "C" fn mark_interrupted(_signal: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
unsafe extern "C" {
    fn signal(signal: i32, handler: usize) -> usize;
}

fn install_signal_handlers() {
    #[cfg(unix)]
    unsafe {
        signal(2, mark_interrupted as *const () as usize);
        signal(15, mark_interrupted as *const () as usize);
    }
}

fn remount_root() -> io::Result<()> {
    // Legacy `remount` cleared cached sudo credentials in its BEGIN block.
    if let Err(error) = run_cleanup("sudo", &["-K"]) {
        eprintln!("Crackle: could not clear sudo credentials: {error}");
    }
    let original = mount_option(Path::new("/"))?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not determine root filesystem mount state",
        )
    })?;
    let mut transaction = RemountTransaction {
        original,
        temporary_apt_mounts: Vec::new(),
        complete: false,
    };

    if original == "ro" {
        println!("remounting rootfs as readwrite");
        for path in ["/var/lib/apt", "/var/cache/apt"] {
            if mount_option(Path::new(path))?.is_none() {
                transaction.temporary_apt_mounts.push(path);
                run("sudo", &["mount", "-t", "tmpfs", "tmpfs", path])?;
                check_interrupted()?;
            }
        }
        run("sudo", &["mount", "-o", "remount,rw", "/"])?;
        check_interrupted()?;
        for path in transaction.temporary_apt_mounts.clone().into_iter().rev() {
            run("sudo", &["umount", path])?;
            transaction
                .temporary_apt_mounts
                .retain(|mounted| *mounted != path);
            check_interrupted()?;
        }
    } else {
        println!("remounting rootfs as readonly");
        run("sudo", &["mount", "-o", "remount,ro", "/"])?;
        check_interrupted()?;
    }
    transaction.restore()?;
    check_interrupted()?;
    run_cleanup("sudo", &["-K"])?;
    transaction.complete = true;
    Ok(())
}

fn check_interrupted() -> io::Result<()> {
    #[cfg(unix)]
    let interrupted = INTERRUPTED.load(Ordering::SeqCst);
    #[cfg(not(unix))]
    let interrupted = false;
    interruption_result(interrupted)
}

fn interruption_result(interrupted: bool) -> io::Result<()> {
    if interrupted {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "operation interrupted; cleaning temporary resources",
        ))
    } else {
        Ok(())
    }
}

struct RemountTransaction {
    original: &'static str,
    temporary_apt_mounts: Vec<&'static str>,
    complete: bool,
}

fn mount_restore_option(original: &str, current: Option<&str>) -> Option<&'static str> {
    if current == Some(original) {
        None
    } else {
        match original {
            "ro" => Some("remount,ro"),
            "rw" => Some("remount,rw"),
            _ => None,
        }
    }
}

impl RemountTransaction {
    fn restore(&mut self) -> io::Result<()> {
        let mut first_error = None;
        match mount_option(Path::new("/")) {
            Ok(current) => {
                if let Some(option) = mount_restore_option(self.original, current)
                    && let Err(error) = run_cleanup("sudo", &["mount", "-o", option, "/"])
                {
                    first_error = Some(error);
                }
            }
            Err(error) => first_error = Some(error),
        }
        let temporary_mounts = std::mem::take(&mut self.temporary_apt_mounts);
        for path in temporary_mounts.into_iter().rev() {
            match run_cleanup("sudo", &["umount", path]) {
                Ok(()) => {}
                Err(error) if first_error.is_none() => {
                    self.temporary_apt_mounts.push(path);
                    first_error = Some(error);
                }
                Err(_) => self.temporary_apt_mounts.push(path),
            }
        }
        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(())
        }
    }
}

impl Drop for RemountTransaction {
    fn drop(&mut self) {
        if self.complete {
            return;
        }
        if let Err(error) = self.restore() {
            eprintln!("Crackle: could not restore the previous remount state: {error}");
        }

        if let Err(error) = run_cleanup("sudo", &["-K"]) {
            eprintln!("Crackle: could not clear sudo credentials: {error}");
        }
    }
}
