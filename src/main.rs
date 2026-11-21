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

fn main() {
    install_signal_handlers();
    if let Err(error) = dispatch() {
        eprintln!("Crackle: {error}");
        std::process::exit(100);
    }
}

fn apt_path(
    variable: &str,
    xdg_variable: &str,
    fallback: &str,
    subdirectory: &str,
) -> io::Result<PathBuf> {
    if let Some(path) = env::var_os(variable) {
        Ok(PathBuf::from(path))
    } else {
        Ok(xdg_path(xdg_variable, fallback)?.join(subdirectory))
    }
}

fn dispatch() -> io::Result<()> {
    check_interrupted()?;
    let mut args: Vec<OsString> = env::args_os().collect();
    let executable = args
        .first()
        .and_then(|path| Path::new(path).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("crackle");
    let executable = executable
        .strip_suffix(".exe")
        .unwrap_or(executable)
        .to_owned();
    args.remove(0);
    if executable == "nix-manager" {
        return nix_manager(&args);
    }
    if COMMAND_NAMES.contains(&executable.as_str()) && executable != "crackle" {
        return helper(&executable, &args);
    }

    let command = args
        .first()
        .and_then(|value| value.to_str())
        .unwrap_or("help");
    let values = &args[1..];
    match command {
        "help" => show_help(),
        "debug" => debug(),
        "setup" => setup(),
        "migrate" => migrate(),
        "rollback" => rollback(values.first().and_then(|value| value.to_str())),
        "clean" => nix_manager(&[OsString::from("clean")]),
        "install" | "remove" | "list" | "update" => {
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

fn state_dir() -> io::Result<PathBuf> {
    Ok(xdg_path("XDG_STATE_HOME", ".local/state")?.join("crackle"))
}

fn pkg_path() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("PKG_PATH") {
        Ok(PathBuf::from(path))
    } else {
        Ok(xdg_path("XDG_DATA_HOME", ".local/share")?.join("crackle"))
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

fn executable_path(program: &str) -> Option<PathBuf> {
    let mut directories: Vec<PathBuf> =
        env::split_paths(&env::var_os("PATH").unwrap_or_default()).collect();
    if let Ok(home) = home() {
        directories.extend([
            home.join(".local/bin"),
            home.join(".nix-profile/bin"),
            PathBuf::from("/nix/var/nix/profiles/default/bin"),
        ]);
    }
    if let Ok(prefix) = prefix() {
        directories.push(prefix.join("bin"));
    }
    directories
        .into_iter()
        .map(|directory| directory.join(program))
        .find(|path| path.is_file())
}

fn effective_path() -> OsString {
    let mut paths: Vec<PathBuf> =
        env::split_paths(&env::var_os("PATH").unwrap_or_default()).collect();
    if let Ok(home) = home() {
        paths.extend([
            home.join(".local/bin"),
            home.join(".nix-profile/bin"),
            PathBuf::from("/nix/var/nix/profiles/default/bin"),
        ]);
    }
    if let Ok(prefix) = prefix() {
        paths.push(prefix.join("bin"));
    }
    env::join_paths(paths).unwrap_or_else(|_| env::var_os("PATH").unwrap_or_default())
}

fn command(program: &str) -> Command {
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

fn show_help() -> io::Result<()> {
    println!(
        "available commands:
  crackle setup
  crackle migrate       download and install the verified ARM64 Rust release
  crackle rollback [ID] restore the most recent (or named) migration backup
  crackle sudo $BIN
  crackle sudok $BIN
  crackle clean
  crackle install $PKG...
  crackle remove $PKG...
  crackle update
  crackle list
  crackle debug
  crackle crack $PKG
  crackle click $PKG...
  crackle bump-version $PKG...
  crackle nuke
  crackle help"
    );
    Ok(())
}

fn debug() -> io::Result<()> {
    let home = home()?;
    let data = xdg_path("XDG_DATA_HOME", ".local/share")?;
    let pkg = pkg_path()?;
    let prefix = prefix()?;
    println!("CRACKLE CONFIG");
    println!("PKG_PATH: '{}'", pkg.display());
    println!("PKG_PREFIX: '{}'", prefix.display());
    println!("PKGS_DIR: '{}'", pkg.display());
    println!(
        "CRACKLECMP: '{}'",
        data.join("bash-completion/completions").display()
    );
    println!("CRACKDIR: '{}'", home.join("packages").display());
    println!("APT CONFIG");
    println!(
        "APTCONFIG: '{}'",
        apt_path("APTCONFIG", "XDG_CONFIG_HOME", ".config", "apt")?.display()
    );
    println!(
        "APTCACHE: '{}'",
        apt_path("APTCACHE", "XDG_CACHE_HOME", ".cache", "apt")?.display()
    );
    println!(
        "APTSTATE: '{}'",
        apt_path("APTSTATE", "XDG_STATE_HOME", ".local/state", "apt")?.display()
    );
    println!(
        "APT_GET: apt-get -o Dir::Cache={} -o Dir::State={}",
        apt_path("APTCACHE", "XDG_CACHE_HOME", ".cache", "apt")?.display(),
        apt_path("APTSTATE", "XDG_STATE_HOME", ".local/state", "apt")?.display()
    );
    println!(
        "DPKGSTATE: '{}'",
        apt_path("DPKGSTATE", "XDG_STATE_HOME", ".local/state", "dpkg")?.display()
    );
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
        "rorw" => println!("{}", mount_option(Path::new("/"))?.unwrap_or("unknown")),
        "rwnix" => println!("{}", mount_option(Path::new("/nix"))?.unwrap_or("unknown")),
        "osvariant" => {
            if let Ok(contents) = fs::read_to_string("/etc/os-release") {
                for line in contents.lines().filter(|line| line.contains("Touch")) {
                    if let Some((_, value)) = line.split_once('=') {
                        println!("{}", value.trim_matches('"'));
                        break;
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
                _ => println!("{}", fallback_triplet()),
            }
        }
        "desktop_trunc" => desktop_trunc(args)?,
        "_crackleopts" => println!(
            "sudo sudok setup migrate rollback clean install remove update debug list crack click bump-version nuke help"
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

fn install_binaries() -> io::Result<()> {
    let executable = env::current_exe()?;
    let bin = prefix()?.join("bin");
    fs::create_dir_all(&bin)?;
    for name in COMMAND_NAMES {
        let target = bin.join(name);
        let temporary = bin.join(format!(".{name}.new"));
        fs::copy(&executable, &temporary)?;
        set_executable(&temporary)?;
        fs::rename(temporary, target)?;
    }
    let completion = xdg_path("XDG_DATA_HOME", ".local/share")?.join("bash-completion/completions");
    fs::create_dir_all(&completion)?;
    let content = include_str!("../share/bash-completion/completions/crackle");
    atomic_write(&completion.join("crackle"), content.as_bytes())?;
    println!("Installed Crackle commands in {}.", bin.display());
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
    let update_nix = is_installed_crackle(&env::current_exe()?, &prefix()?);
    install_binaries()?;
    setup_environment()?;
    if update_nix {
        nix_manager(&[OsString::from("update")])?;
    }
    Ok(())
}

fn is_installed_crackle(executable: &Path, prefix: &Path) -> bool {
    executable == prefix.join("bin/crackle")
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

fn setup_environment() -> io::Result<()> {
    let root_status = mount_option(Path::new("/"))?.unwrap_or("unknown");
    let nix_status = mount_option(Path::new("/nix"))?.unwrap_or("unknown");
    let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let is_ubuntu = os_release
        .lines()
        .any(|line| line == "ID=ubuntu" || line == "ID=\"ubuntu\"");
    let is_touch = os_release.contains("Touch");
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
                format!("{} is currently not supported", os_release_id(&os_release)),
            )
        }
    })?;
    let mut apt = None;

    match plan {
        SetupPlan::TouchUserSpaceApt => {
            apt = Some(AptWorkspace::create()?);
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
            if !available("curl") {
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
    if is_touch {
        println!(
            "Set up complete. Log in again, or run:\n  . {}/.nix-profile/etc/profile.d/nix.sh",
            home()?.display()
        );
    } else {
        println!("Set up complete.");
    }
    Ok(())
}

fn os_release_id(contents: &str) -> &str {
    contents
        .lines()
        .find_map(|line| line.strip_prefix("ID="))
        .unwrap_or("this operating system")
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
    fn create() -> io::Result<Self> {
        let unique = format!("crackle-apt-{}", std::process::id());
        let cache = apt_path("APTCACHE", "XDG_CACHE_HOME", ".cache", "apt")?;
        let state = apt_path("APTSTATE", "XDG_STATE_HOME", ".local/state", "apt")?;
        let dpkg = apt_path("DPKGSTATE", "XDG_STATE_HOME", ".local/state", "dpkg")?;
        let config = apt_path("APTCONFIG", "XDG_CONFIG_HOME", ".config", "apt")?;
        let data = pkg_path()?;
        let base = data.join(&unique);
        let mut owned_paths = Vec::new();
        for path in [&cache, &state, &dpkg, &config, &base] {
            if !path.exists() {
                owned_paths.push(path.to_path_buf());
            }
            fs::create_dir_all(path)?;
        }
        let status = Path::new("/var/lib/dpkg/status");
        if status.is_file() && !dpkg.join("status").exists() {
            fs::copy(status, dpkg.join("status"))?;
        }
        let sources = config.join("sources.list.d");
        let trusted = config.join("trusted.gpg.d");
        fs::create_dir_all(&sources)?;
        fs::create_dir_all(&trusted)?;
        let release = fs::read_to_string("/etc/os-release").unwrap_or_default();
        if release.contains("focal") {
            let source = Path::new("/etc/apt/sources.list");
            if source.is_file() {
                copy_or_link(source, &config.join("sources.list"))?;
            }
        } else if release.contains("noble") {
            copy_directory_files(Path::new("/etc/apt/sources.list.d"), &sources, "")?;
        }
        copy_directory_files(Path::new("/etc/apt/trusted.gpg.d"), &trusted, "")?;
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
        std::os::unix::fs::symlink(source, &destination)?;
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

fn ensure_curl_xz(apt: &mut AptWorkspace) -> io::Result<()> {
    let mut missing = Vec::new();
    if !available("curl") {
        missing.push("curl");
    }
    if !available("xz") {
        missing.push("xz-utils");
    }
    if missing.is_empty() {
        return Ok(());
    }
    run_command(apt.apt_get().args(["update"])).and_then(|status| {
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("apt-get update failed"))
        }
    })?;
    let mut command = apt.apt_get();
    command.args(["install", "--download-only", "-y"]);
    command.args(missing);
    let status = run_command(&mut command)?;
    if !status.success() {
        return Err(io::Error::other("apt-get download-only install failed"));
    }
    let archives = apt.cache.join("archives");
    let package_dir = apt.data.join("packages");
    fs::create_dir_all(&package_dir)?;
    for entry in fs::read_dir(archives)? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "deb") {
            let output = package_dir.join(
                path.file_stem()
                    .ok_or_else(|| io::Error::other("invalid .deb filename"))?,
            );
            run_os(
                "dpkg-deb",
                &[
                    OsString::from("-x"),
                    path.into_os_string(),
                    output.clone().into_os_string(),
                ],
            )?;
            for name in ["curl", "xz"] {
                let source = output.join("usr/bin").join(name);
                let destination = prefix()?.join("bin").join(name);
                if source.is_file() && !destination.exists() {
                    fs::create_dir_all(destination.parent().unwrap())?;
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(source, &destination)?;
                    apt.temporary_links.push(destination.clone());
                    #[cfg(not(unix))]
                    fs::copy(source, destination)?;
                }
            }
        }
    }
    Ok(())
}

fn setup_nix_home_manager() -> io::Result<()> {
    if !available("nix") {
        println!("Installing Nix...");
        let installer = env::temp_dir().join(format!("crackle-nix-install-{}", std::process::id()));
        let status = run_command(
            command("curl")
                .args([
                    "--proto",
                    "=https",
                    "--tlsv1.2",
                    "--fail",
                    "--location",
                    "https://nixos.org/nix/install",
                    "--output",
                ])
                .arg(&installer),
        )?;
        if !status.success() {
            return Err(io::Error::other("failed to download the Nix installer"));
        }
        let status = run_command(
            command("sh")
                .arg(&installer)
                .args(["--no-daemon", "--no-channel-add"]),
        )?;
        let _ = fs::remove_file(installer);
        if !status.success() {
            return Err(io::Error::other("Nix installer failed"));
        }
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
                    .arg(format!("https://nixos.org/channels/nixos-{NIX_VERSION}")),
            )?;
            if !output.success() {
                return Err(io::Error::other("failed to configure the Nix channel"));
            }
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
            let backup = PathBuf::from(format!("{}.backup", path.display()));
            fs::copy(&path, &backup)?;
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
            config::update_packages(&path, "home", &additions, &removals, false)?;
            if let Err(error) = run("home-manager", &["build", "--no-out-link"]) {
                restore_config(&path, &backup)?;
                return Err(error);
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
                run("nix-collect-garbage", &["-d"])?;
            }
        }
        "clean" => {
            run("nix-collect-garbage", &["-d"])?;
            run("nix-store", &["--optimise"])?;
        }
        "update" => {
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
            run("nix-channel", &["--update"])?;
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

fn apt_workspace() -> io::Result<AptWorkspace> {
    AptWorkspace::create()
}

fn setup_repositories(apt: &AptWorkspace) -> io::Result<()> {
    let archive = apt.base.join("cracklebin.tar.gz");
    let status = run_command(
        command("curl")
            .args([
                "--disable",
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                CRACKLE_BIN_ARCHIVE,
                "--output",
            ])
            .arg(&archive),
    )?;
    if !status.success() {
        return Err(io::Error::other(
            "failed to download Crackle's Apt repository configuration",
        ));
    }

    let listing = capture_command(command("tar").args(["-tzf"]).arg(&archive))?;
    if !listing.status.success() {
        return Err(io::Error::other(
            "could not read Crackle's Apt repository archive",
        ));
    }
    let members = String::from_utf8_lossy(&listing.stdout);
    let preferences = apt.config.join("preferences.d");
    let sources = apt.config.join("sources.list.d");
    let library = prefix()?.join("lib/crackle");
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
            let status = run_command(
                command("curl")
                    .args([
                        "--fail",
                        "--location",
                        "--silent",
                        "--show-error",
                        url,
                        "--output",
                    ])
                    .arg(&key),
            )?;
            if !status.success() {
                return Err(io::Error::other(format!(
                    "failed to download the {name} Apt signing key"
                )));
            }
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
    let mut apt = apt_workspace()?;
    ensure_curl_xz(&mut apt)?;
    clean_apt(&apt)?;
    setup_repositories(&apt)?;
    let mut update = apt.apt_get();
    update.arg("update");
    if !run_command(&mut update)?.success() {
        return Err(io::Error::other("apt-get update failed"));
    }
    let mut download = apt.apt_get();
    download.args(["install", "--download-only", "-y"]);
    download.args(args);
    if !run_command(&mut download)?.success() {
        return Err(io::Error::other("apt-get download-only install failed"));
    }
    let package_name = args[0].to_string_lossy();
    let destination = home()?.join("packages").join(package_name.as_ref());
    fs::create_dir_all(&destination)?;
    for entry in fs::read_dir(apt.cache.join("archives"))? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "deb") {
            run_os(
                "dpkg-deb",
                &[
                    OsString::from("-x"),
                    path.into_os_string(),
                    destination.clone().into_os_string(),
                ],
            )?;
        }
    }
    println!("Extracted packages to {}.", destination.display());
    Ok(())
}

fn click(args: &[OsString], bump: bool) -> io::Result<()> {
    if args.first().is_some_and(|value| value == "debug") {
        return click_debug();
    }
    ensure_not_root()?;
    if bump {
        let root = env::var_os("ROOT")
            .map(PathBuf::from)
            .unwrap_or(env::current_dir()?);
        let manifest = root.join("manifest.json");
        return bump_version(args, &root, &manifest);
    }
    if !available("click") {
        return Err(io::Error::new(io::ErrorKind::NotFound, "missing click"));
    }
    if args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing package name",
        ));
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
    let triplet = env::var("ARCH_TRIPLET").unwrap_or_else(|_| {
        command_output("dpkg-architecture", &["-qDEB_HOST_MULTIARCH"])
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fallback_triplet().to_string())
    });
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
    let mut apt = apt_workspace()?;
    ensure_curl_xz(&mut apt)?;
    check_connectivity()?;
    clean_apt(&apt)?;
    setup_repositories(&apt)?;
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
    for entry in fs::read_dir(apt.cache.join("archives"))? {
        let deb = entry?.path();
        if deb.extension().is_some_and(|extension| extension == "deb") {
            run_os(
                "dpkg-deb",
                &[
                    OsString::from("-x"),
                    deb.into_os_string(),
                    build.clone().into_os_string(),
                ],
            )?;
        }
    }
    prepare_click_manifest(&root, &build, &install, &click_bin, args)?;
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
    copy_tree_if_exists(&build.join("usr/share"), &install.join("usr/share"))?;
    let adapt = build.join("adapt.sh");
    if manifest.is_file() && adapt.is_file() {
        let package_names = args
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
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
                .env("PKG", package_names),
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
    if env::var_os("CI").is_none() {
        run_os(
            "click",
            &[OsString::from("build"), install.into_os_string()],
        )?;
    }
    Ok(())
}

fn click_debug() -> io::Result<()> {
    let root = env::var_os("ROOT")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?);
    let install = env::var_os("INSTALL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("install"));
    let triplet = env::var("ARCH_TRIPLET").unwrap_or_else(|_| fallback_triplet().to_string());
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
    let source = if manifest.is_file() {
        manifest
    } else if template.is_file() {
        template
    } else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "manifest.json or manifest.json.in is missing",
        ));
    };
    if !root.join("manifest.json.in").is_file() {
        copy_tree_if_exists(&root.join("pkg.d"), build)?;
        for name in ["apparmor.json", "manifest.json"] {
            let path = root.join(name);
            if path.is_file() {
                fs::copy(path, build.join(name))?;
            }
        }
        if let Some(name) = packages.first() {
            let manifest_text = fs::read_to_string(build.join("manifest.json"))?;
            atomic_write(
                &build.join("manifest.json"),
                manifest_text
                    .replace("@pkg@", &name.to_string_lossy())
                    .as_bytes(),
            )?;
        }
    }
    fs::create_dir_all(install)?;
    let name = packages
        .first()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "package".to_string());
    let manifest_text = fs::read_to_string(&source)?.replace("@pkg@", &name);
    atomic_write(&install.join("manifest.json"), manifest_text.as_bytes())?;
    for entry in fs::read_dir(build)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            fs::copy(&path, install.join(path.file_name().unwrap()))?;
        }
    }
    if !root.join("manifest.json.in").is_file() {
        let package_script = build.join("pkg.sh");
        if package_script.is_file() {
            fs::create_dir_all(click_bin)?;
            fs::copy(package_script, click_bin.join(format!("{name}.sh")))?;
        }
    }
    Ok(())
}

fn copy_tree_if_exists(source: &Path, destination: &Path) -> io::Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
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

fn bump_version(args: &[OsString], root: &Path, manifest: &Path) -> io::Result<()> {
    let mut apt = apt_workspace()?;
    ensure_curl_xz(&mut apt)?;
    check_connectivity()?;
    clean_apt(&apt)?;
    setup_repositories(&apt)?;
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
    let version = candidate
        .split(['~', '+'])
        .next()
        .unwrap_or(candidate)
        .split_once(':')
        .map(|(_, version)| version)
        .unwrap_or(candidate);
    let text = fs::read_to_string(manifest)?;
    let current = text
        .lines()
        .find(|line| line.contains("version"))
        .and_then(|line| line.split('"').nth(1))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "manifest version not found"))?;
    println!("available version: {version}");
    println!("current version: {current}");
    if current != version {
        let updated = text.replacen(current, version, 1);
        atomic_write(&root.join("manifest.json"), updated.as_bytes())?;
        println!("Updated manifest.json.");
    }
    Ok(())
}

fn sudo_command(args: &[&str]) -> io::Result<()> {
    println!("this action requires sudo access");
    run("sudo", args)
}

fn sudo_link(args: &[OsString], enable: bool) -> io::Result<()> {
    if args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "missing argument",
        ));
    }
    let mut destination = PathBuf::from("/root/.local/bin");
    if Path::new("/snap/bin").is_dir() {
        destination = PathBuf::from("/snap/bin");
    } else if enable {
        sudo_command(&["mkdir", "-p", "/root/.local/bin"])?;
        configure_sudo_profile()?;
    }
    for name in args {
        let name = name.to_string_lossy();
        if enable {
            let resolved = capture_command(Command::new("which").arg(name.as_ref()))?;
            if !resolved.status.success() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("command '{name}' was not found"),
                ));
            }
            let path = PathBuf::from(String::from_utf8_lossy(&resolved.stdout).trim());
            let canonical = fs::canonicalize(path)?;
            sudo_command(&[
                "ln",
                "-sfn",
                canonical.to_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 executable path")
                })?,
                destination.join(name.as_ref()).to_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 link path")
                })?,
            ])?;
        } else {
            sudo_command(&[
                "rm",
                "-f",
                destination.join(name.as_ref()).to_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 link path")
                })?,
            ])?;
        }
    }
    run("sudo", &["-k"])
}

fn configure_sudo_profile() -> io::Result<()> {
    let profile = "/root/.profile";
    sudo_command(&["touch", profile])?;
    let output = capture_command(command("sudo").args(["cat", profile]))?;
    if !output.status.success() {
        return Err(io::Error::other("could not read /root/.profile"));
    }
    let mut contents = output.stdout;
    let path_line = b"export PATH=/root/.local/bin:$PATH";
    if contents
        .split(|byte| *byte == b'\n')
        .any(|line| line.strip_suffix(b"\r").unwrap_or(line) == path_line)
    {
        return Ok(());
    }
    if !contents.is_empty() && !contents.ends_with(b"\n") {
        contents.push(b'\n');
    }
    contents.extend_from_slice(path_line);
    contents.push(b'\n');
    let mut child = command("sudo")
        .args(["tee", profile])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("could not open sudo tee input"))?
        .write_all(&contents)?;
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("could not update /root/.profile"))
    }
}

fn nuke() -> io::Result<()> {
    ensure_not_root()?;
    println!("Removing Crackle, Nix, and home-manager state.");
    for channel in ["home-manager", "nixpkgs"] {
        if let Err(error) = run("nix-channel", &["--remove", channel]) {
            eprintln!("Crackle: could not remove Nix channel '{channel}': {error}");
        }
    }
    if mount_option(Path::new("/"))? == Some("rw") {
        run("sudo", &["rm", "-rf", "/nix"])?;
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
        )?;
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
        let target = home.join(relative);
        if target.is_dir() {
            fs::remove_dir_all(target)?;
        } else if target.exists() {
            fs::remove_file(target)?;
        }
    }
    let prefix = prefix()?;
    for relative in [
        "bin/crackle",
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
        "share/bash-completion/completions/crackle",
    ] {
        let path = prefix.join(relative);
        if path.is_file()
            || fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            fs::remove_file(path)?;
        }
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
            let _ = fs::remove_file(nix_config.join(name));
        }
        let _ = fs::remove_dir(nix_config);
    }
    let _ = fs::remove_dir_all(prefix.join("lib/crackle"));
    Ok(())
}

fn migrate() -> io::Result<()> {
    ensure_not_root()?;
    migration::migrate(&prefix()?, &state_dir()?, &home()?)
}

fn rollback(id: Option<&str>) -> io::Result<()> {
    ensure_not_root()?;
    migration::rollback(&prefix()?, &state_dir()?, id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reads_mount_flags_without_confusing_rootfs_with_root_mount() {
        let mounts = "rootfs / rootfs rw 0 0\n/dev/mmcblk0p2 / ext4 ro,relatime 0 0\n/dev/loop0 /nix ext4 rw,relatime 0 0\n";
        assert_eq!(mount_option_from(mounts, Path::new("/")), Some("ro"));
        assert_eq!(mount_option_from(mounts, Path::new("/nix")), Some("rw"));
        assert_eq!(mount_option_from(mounts, Path::new("/missing")), None);
    }

    #[test]
    fn remount_restoration_matches_both_original_mount_modes() {
        assert_eq!(mount_restore_option("ro", Some("rw")), Some("remount,ro"));
        assert_eq!(mount_restore_option("ro", Some("ro")), None);
        assert_eq!(mount_restore_option("rw", Some("ro")), Some("remount,rw"));
        assert_eq!(mount_restore_option("rw", Some("rw")), None);
    }

    #[test]
    fn readonly_touch_uses_userspace_apt_without_remounting_root() {
        assert_eq!(
            setup_plan(true, true, "ro", "rw", false).unwrap(),
            SetupPlan::TouchUserSpaceApt
        );
        assert_eq!(
            setup_plan(true, true, "ro", "rw", true).unwrap(),
            SetupPlan::TouchNixReady
        );
        assert!(
            setup_plan(true, true, "ro", "ro", false).is_err(),
            "a read-only /nix must not trigger a system remount"
        );
    }

    #[test]
    fn installed_setup_keeps_the_legacy_nix_update_path() {
        let prefix = Path::new("prefix");
        assert!(is_installed_crackle(&prefix.join("bin/crackle"), prefix));
        assert!(!is_installed_crackle(
            Path::new("temporary-crackle"),
            prefix
        ));
    }

    #[test]
    fn apt_source_key_paths_follow_the_configured_apt_workspace() {
        let contents = "deb [signed-by=$HOME/.config/apt/trusted.gpg.d/microsoft.asc] https://example.invalid stable main\n# $HOME/data\n";
        assert_eq!(
            resolve_apt_source_paths(
                contents,
                Path::new("custom/apt-config"),
                Path::new("user-home")
            ),
            "deb [signed-by=custom/apt-config/trusted.gpg.d/microsoft.asc] https://example.invalid stable main\n# user-home/data\n"
        );
    }

    #[test]
    fn helper_outputs_and_package_architecture_match_legacy_behavior() {
        assert_eq!(
            sudo_debug_output(),
            "SUDO CONFIG\nSUDO_CONFIG: '/root/.config'\nSUDO_BIN: '/root/.local/bin'\nSUDO_STATE: '/root/.local/state'\n"
        );
        assert_eq!(
            desktop_entry("[Desktop Entry]\nName=Demo\n\n[Other]\n"),
            "[Desktop Entry]\nName=Demo\n\n"
        );
        assert_eq!(
            package_with_arch(&OsString::from("vim"), "arm64"),
            OsString::from("vim:arm64")
        );
        assert_eq!(
            package_with_arch(&OsString::from("font:all"), "arm64"),
            OsString::from("font:all")
        );
    }

    #[test]
    fn interrupted_operation_drops_and_cleans_apt_workspace() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("crackle-apt-test-{unique}"));
        let base = root.join("workspace");
        let owned = root.join("owned");
        let link = root.join("temporary-tool");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&owned).unwrap();
        fs::write(&link, b"temporary").unwrap();
        let workspace = AptWorkspace {
            base,
            cache: root.join("cache"),
            state: root.join("state"),
            dpkg: root.join("dpkg"),
            config: root.join("config"),
            data: root.clone(),
            temporary_links: vec![link.clone()],
            owned_paths: vec![owned.clone()],
        };

        assert_eq!(
            interruption_result(true).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        drop(workspace);
        assert!(!root.join("workspace").exists());
        assert!(!owned.exists());
        assert!(!link.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
