use super::*;
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InstallLayout {
    /// The normal install: `bin/crackle` plus all helper names and completion.
    Standard,
    /// Coexists with a legacy Bash/Python Crackle installation: installs
    /// `bin/crackle-rust`, only fills missing helpers, and uses isolated paths.
    SideBySide,
}

impl InstallLayout {
    pub(super) fn main_name(self) -> &'static str {
        match self {
            Self::Standard => "crackle",
            Self::SideBySide => "crackle-rust",
        }
    }

    pub(super) fn data_dir_name(self) -> &'static str {
        match self {
            Self::Standard => "crackle",
            Self::SideBySide => "crackle-rust",
        }
    }

    pub(super) fn is_side_by_side(self) -> bool {
        matches!(self, Self::SideBySide)
    }
}

pub(super) fn invoked_name() -> String {
    let name = env::args_os()
        .next()
        .and_then(|path| Path::new(&path).file_name().map(|name| name.to_owned()))
        .and_then(|name| name.to_str().map(str::to_owned))
        .unwrap_or_else(|| "crackle".to_owned());
    name.strip_suffix(".exe").unwrap_or(&name).to_owned()
}

pub(super) fn legacy_install_present(prefix: &Path) -> bool {
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

pub(super) fn resolve_layout_from(
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

pub(super) fn resolve_layout(invoked: &str) -> io::Result<InstallLayout> {
    let override_value = env::var("CRACKLE_INSTALL_LAYOUT").ok();
    let legacy_present = legacy_install_present(&prefix()?);
    Ok(resolve_layout_from(
        invoked,
        legacy_present,
        override_value.as_deref(),
    ))
}

pub(super) fn library_dir(layout: InstallLayout, prefix: &Path) -> PathBuf {
    prefix.join("lib").join(layout.data_dir_name())
}

pub(super) fn nuke_allowed(
    layout: InstallLayout,
    legacy_present: bool,
    forced: bool,
) -> io::Result<()> {
    if layout.is_side_by_side() && legacy_present && !forced {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "legacy Crackle is installed; nuke removes shared Nix/home-manager state. Re-run with CRACKLE_FORCE=1 to force a full nuke.",
        ));
    }
    Ok(())
}

pub(super) fn apt_path(
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

pub(super) fn home() -> io::Result<PathBuf> {
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

pub(super) fn xdg_path(variable: &str, fallback: &str) -> io::Result<PathBuf> {
    if let Some(path) = env::var_os(variable) {
        return Ok(PathBuf::from(path));
    }
    Ok(home()?.join(fallback))
}

pub(super) fn prefix() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("PKG_PREFIX") {
        Ok(PathBuf::from(path))
    } else {
        Ok(home()?.join(".local"))
    }
}

pub(super) fn state_dir(layout: InstallLayout) -> io::Result<PathBuf> {
    Ok(xdg_path("XDG_STATE_HOME", ".local/state")?.join(layout.data_dir_name()))
}

pub(super) fn completion_dir() -> io::Result<PathBuf> {
    Ok(xdg_path("XDG_DATA_HOME", ".local/share")?.join("bash-completion/completions"))
}

pub(super) fn completion_file() -> io::Result<PathBuf> {
    Ok(completion_dir()?.join("crackle"))
}

pub(super) fn pkg_path(layout: InstallLayout) -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("PKG_PATH") {
        Ok(PathBuf::from(path))
    } else {
        Ok(xdg_path("XDG_DATA_HOME", ".local/share")?.join(layout.data_dir_name()))
    }
}

pub(super) fn command_output(program: &str, args: &[&str]) -> io::Result<Output> {
    capture_command(command(program).args(args))
}

pub(super) fn run(program: &str, args: &[&str]) -> io::Result<()> {
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

pub(super) fn run_os(program: &str, args: &[OsString]) -> io::Result<()> {
    let status = run_command(command(program).args(args))?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "`{program}` exited with {status}"
        )))
    }
}

pub(super) fn run_command(command: &mut Command) -> io::Result<std::process::ExitStatus> {
    let status = command.status()?;
    if !status.success() {
        // Let the caller report the command's own, specific failure instead
        // of masking it behind a generic "operation interrupted" error - a
        // pending interrupt is only relevant to stop a successful step's
        // follow-on work, not to explain away a real command failure.
        return Ok(status);
    }
    check_interrupted()?;
    Ok(status)
}

pub(super) fn run_cleanup(program: &str, args: &[&str]) -> io::Result<()> {
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

pub(super) fn capture_command(command: &mut Command) -> io::Result<Output> {
    let output = command.output()?;
    if output.status.success() {
        check_interrupted()?;
    }
    Ok(output)
}

pub(super) fn available(program: &str) -> bool {
    executable_path(program).is_some()
}

/// Search order mirrors legacy `check_path`: Crackle's own bin and the Nix
/// profile come first, then PATH. This prevents a broken `/snap/bin` tool from
/// shadowing a working or freshly extracted one.
pub(super) fn search_directories() -> Vec<PathBuf> {
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

pub(super) fn executable_path(program: &str) -> Option<PathBuf> {
    search_directories()
        .into_iter()
        .map(|directory| directory.join(program))
        .find(|path| path.is_file())
}

pub(super) fn effective_path() -> OsString {
    env::join_paths(search_directories())
        .unwrap_or_else(|_| env::var_os("PATH").unwrap_or_default())
}

pub(super) fn curl_candidates() -> Vec<PathBuf> {
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
pub(super) fn working_curl() -> Option<PathBuf> {
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

pub(super) fn curl_available() -> bool {
    working_curl().is_some()
}

/// Put the directory of a working curl first so the Nix installer's internal
/// `curl` lookup cannot pick a broken Snap wrapper.
pub(super) fn path_with_working_curl() -> OsString {
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

pub(super) fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

pub(super) fn remove_dir_if_exists(path: &Path) -> io::Result<()> {
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

pub(super) fn is_root() -> bool {
    command_output("id", &["-u"])
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
}

pub(super) fn ensure_not_root() -> io::Result<()> {
    if is_root() && env::var_os("ROOT").is_none() {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "please don't run me as root",
        ))
    } else {
        Ok(())
    }
}

#[cfg(unix)]
pub(super) fn set_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
pub(super) fn set_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub(super) fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
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
