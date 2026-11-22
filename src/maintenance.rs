use super::*;
use std::io::Write;

pub(super) fn sudo_command(args: &[&str]) -> io::Result<()> {
    println!("this action requires sudo access");
    run("sudo", args)
}

const SUDO_DEFAULT_BIN: &str = "/root/.local/bin";
const SUDO_SNAP_BIN: &str = "/snap/bin";
const SUDO_LINK_STATE_DIR: &str = "/root/.local/state/crackle/sudo-links";

#[derive(Debug, PartialEq, Eq)]
struct SudoLinkRecord {
    link: PathBuf,
    target: PathBuf,
}

fn sudo_link_dirs() -> [PathBuf; 2] {
    [
        PathBuf::from(SUDO_DEFAULT_BIN),
        PathBuf::from(SUDO_SNAP_BIN),
    ]
}

fn sudo_link_state_dir() -> PathBuf {
    PathBuf::from(SUDO_LINK_STATE_DIR)
}

fn sudo_link_name(name: &OsString) -> io::Result<&str> {
    let name = name
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "binary name is not UTF-8"))?;
    if name.is_empty()
        || name == "."
        || name == ".."
        || name
            .bytes()
            .any(|byte| matches!(byte, b'/' | b'\\' | b'\0' | b'\n' | b'\r' | b'\t'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "binary name must be a single path component",
        ));
    }
    Ok(name)
}

fn parse_sudo_link_record(
    name: &str,
    contents: &str,
    allowed_dirs: &[PathBuf],
) -> io::Result<SudoLinkRecord> {
    let mut lines = contents.lines();
    let link = lines
        .next()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing sudo link path"))?;
    let target = lines
        .next()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing sudo link target"))?;
    let parent = link.parent();
    if lines.next().is_some()
        || !link.is_absolute()
        || !target.is_absolute()
        || link.file_name().and_then(|value| value.to_str()) != Some(name)
        || !allowed_dirs
            .iter()
            .any(|directory| parent == Some(directory.as_path()))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid sudo link ownership record",
        ));
    }
    Ok(SudoLinkRecord { link, target })
}

fn read_sudo_link_record(
    name: &str,
    allowed_dirs: &[PathBuf],
) -> io::Result<Option<SudoLinkRecord>> {
    let path = sudo_link_state_dir().join(name);
    let path = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 record path"))?;
    let output = sudo_capture(&[
        "sh",
        "-c",
        "if [ -f \"$1\" ]; then cat -- \"$1\"; else exit 44; fi",
        "sh",
        path,
    ])?;
    if output.status.code() == Some(44) {
        return Ok(None);
    }
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "could not read sudo link ownership record for '{name}'"
        )));
    }
    let contents = String::from_utf8(output.stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    parse_sudo_link_record(name, &contents, allowed_dirs).map(Some)
}

fn sudo_link_matches_record(
    record: Option<&SudoLinkRecord>,
    name: &str,
    link: &Path,
    target: &Path,
    allowed_dirs: &[PathBuf],
) -> bool {
    record.is_some_and(|record| {
        record.link == link
            && record.target == target
            && record.link.file_name().and_then(|value| value.to_str()) == Some(name)
            && allowed_dirs
                .iter()
                .any(|directory| record.link.parent() == Some(directory.as_path()))
    })
}

fn write_sudo_link_record(name: &str, link: &Path, target: &Path) -> io::Result<()> {
    let state_dir = sudo_link_state_dir();
    let record_path = state_dir.join(name);
    let link = link
        .to_str()
        .filter(|value| !value.contains('\n') && !value.contains('\r'))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid sudo link path"))?;
    let target = target
        .to_str()
        .filter(|value| !value.contains('\n') && !value.contains('\r'))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid sudo link target"))?;
    let record_path = record_path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 record path"))?;
    let temporary = state_dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let temporary = temporary
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 temporary path"))?;
    sudo_command(&["mkdir", "-p", SUDO_LINK_STATE_DIR])?;
    let mut child = command("sudo")
        .args(["tee", "--", temporary])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("could not open sudo record input"))?
        .write_all(format!("{link}\n{target}\n").as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        return Err(io::Error::other(
            "could not write sudo link ownership record",
        ));
    }
    sudo_command(&["mv", "-f", "--", temporary, record_path])
}

fn sudo_capture(args: &[&str]) -> io::Result<Output> {
    let output = command("sudo")
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?
        .wait_with_output()?;
    check_interrupted()?;
    Ok(output)
}

fn sudo_path_test(path: &Path, expression: &str) -> io::Result<bool> {
    let path = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 sudo link path"))?;
    let script = format!("if {expression}; then exit 0; else exit 3; fi");
    let status = run_command(command("sudo").args(["sh", "-c", &script, "sh", path]))?;
    match status.code() {
        Some(0) => Ok(true),
        Some(3) => Ok(false),
        _ => Err(io::Error::other(format!(
            "could not inspect sudo link '{path}'"
        ))),
    }
}

fn sudo_readlink(path: &Path) -> io::Result<PathBuf> {
    let path = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 sudo link path"))?;
    let output = command("sudo")
        .args(["readlink", "--", path])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?
        .wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "could not read sudo link target for '{path}'"
        )));
    }
    let target = String::from_utf8(output.stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(PathBuf::from(target.trim_end_matches(['\r', '\n'])))
}

pub(super) fn sudo_link(args: &[OsString], enable: bool) -> io::Result<()> {
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

fn enable_sudo(args: &[OsString]) -> io::Result<()> {
    let destination = if Path::new("/snap/bin").is_dir() {
        unconfigure_sudo_profile()?;
        PathBuf::from("/snap/bin")
    } else {
        configure_sudo_profile()?;
        PathBuf::from(SUDO_DEFAULT_BIN)
    };
    for name in args {
        let name = sudo_link_name(name)?;
        let resolved = capture_command(Command::new("which").arg(name))?;
        if !resolved.status.success() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("command '{name}' was not found"),
            ));
        }
        let path = PathBuf::from(String::from_utf8_lossy(&resolved.stdout).trim());
        let canonical = fs::canonicalize(path)?;
        let link = destination.join(name);
        if sudo_path_test(&link, "[ -e \"$1\" ] || [ -L \"$1\" ]")? {
            eprintln!(
                "Crackle: {} already exists; it was not registered as a Crackle sudo link.",
                link.display()
            );
            continue;
        }
        let target = canonical.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 executable path")
        })?;
        let link_path = link
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 link path"))?;
        sudo_command(&["ln", "-s", "--", target, link_path])?;
        if let Err(error) = write_sudo_link_record(name, &link, &canonical) {
            let rollback = sudo_command(&["rm", "--", link_path]);
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(io::Error::other(format!(
                    "could not record sudo link ({error}); rollback failed ({rollback_error})"
                ))),
            };
        }
    }
    run("sudo", &["-k"])
}

fn disable_sudo(args: &[OsString]) -> io::Result<()> {
    let state_dir = sudo_link_state_dir();
    let allowed_dirs = sudo_link_dirs();
    for name in args {
        let name = sudo_link_name(name)?;
        let record = read_sudo_link_record(name, &allowed_dirs)?;
        let Some(record) = record else {
            eprintln!("Crackle: no tracked sudo link for '{name}'; leaving it untouched.");
            continue;
        };
        if !sudo_path_test(&record.link, "[ -e \"$1\" ] || [ -L \"$1\" ]")? {
            sudo_command(&[
                "rm",
                "--",
                state_dir.join(name).to_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 record path")
                })?,
            ])?;
            continue;
        }
        if !sudo_path_test(&record.link, "[ -L \"$1\" ]")? {
            eprintln!(
                "Crackle: {} is no longer a symlink; leaving it untouched.",
                record.link.display()
            );
            continue;
        }
        let target = sudo_readlink(&record.link)?;
        if !sudo_link_matches_record(Some(&record), name, &record.link, &target, &allowed_dirs) {
            eprintln!(
                "Crackle: {} no longer matches its recorded target; leaving it untouched.",
                record.link.display()
            );
            continue;
        }
        let path = record
            .link
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 link path"))?;
        sudo_command(&["rm", "--", path])?;
        sudo_command(&[
            "rm",
            "--",
            state_dir.join(name).to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 record path")
            })?,
        ])?;
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

pub(super) fn nuke() -> io::Result<()> {
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

pub(super) fn migrate() -> io::Result<()> {
    ensure_not_root()?;
    migration::migrate(
        &prefix()?,
        &xdg_path("XDG_DATA_HOME", ".local/share")?,
        &state_dir(InstallLayout::Standard)?,
    )
}

pub(super) fn rollback(id: Option<&str>) -> io::Result<()> {
    ensure_not_root()?;
    migration::rollback(
        &prefix()?,
        &xdg_path("XDG_DATA_HOME", ".local/share")?,
        &state_dir(InstallLayout::Standard)?,
        id,
    )
}

#[cfg(test)]
#[path = "tests/maintenance_tests.rs"]
mod tests;

#[cfg(unix)]
extern "C" fn mark_interrupted(_signal: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
unsafe extern "C" {
    fn signal(signal: i32, handler: usize) -> usize;
}

pub(super) fn install_signal_handlers() {
    #[cfg(unix)]
    unsafe {
        signal(2, mark_interrupted as *const () as usize);
        signal(15, mark_interrupted as *const () as usize);
    }
}

pub(super) fn remount_root() -> io::Result<()> {
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

pub(super) fn check_interrupted() -> io::Result<()> {
    #[cfg(unix)]
    let interrupted = INTERRUPTED.load(Ordering::SeqCst);
    #[cfg(not(unix))]
    let interrupted = false;
    interruption_result(interrupted)
}

pub(super) fn interruption_result(interrupted: bool) -> io::Result<()> {
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

pub(super) fn mount_restore_option(original: &str, current: Option<&str>) -> Option<&'static str> {
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
