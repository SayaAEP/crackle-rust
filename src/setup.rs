use super::*;

pub(super) fn setup() -> io::Result<()> {
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

pub(super) fn is_installed_crackle(executable: &Path, prefix: &Path) -> bool {
    let bin = prefix.join("bin");
    executable
        .parent()
        .is_some_and(|parent| parent == bin.as_path())
}

/// Parsed `/etc/os-release`. The target device (Ubuntu Touch 24.04) reports
/// `ID=ubuntu`, `VARIANT=Touch`, `VARIANT_ID=touch`, `VERSION_CODENAME=noble`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct OsRelease {
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
    pub(super) fn parse(contents: &str) -> Self {
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

    pub(super) fn is_ubuntu(&self) -> bool {
        self.id.eq_ignore_ascii_case("ubuntu")
            || self.id.eq_ignore_ascii_case("ubuntu-touch")
            || self
                .id_like
                .split_whitespace()
                .any(|part| part.eq_ignore_ascii_case("ubuntu"))
            || self.name.to_lowercase().contains("ubuntu")
    }

    pub(super) fn is_touch(&self) -> bool {
        self.variant.eq_ignore_ascii_case("touch")
            || self.variant_id.eq_ignore_ascii_case("touch")
            || self.id.eq_ignore_ascii_case("ubuntu-touch")
            || self.name.to_lowercase().contains("touch")
            || self.pretty_name.to_lowercase().contains("touch")
    }

    pub(super) fn codename(&self) -> Option<&str> {
        if !self.version_codename.is_empty() {
            Some(&self.version_codename)
        } else if !self.ubuntu_codename.is_empty() {
            Some(&self.ubuntu_codename)
        } else {
            None
        }
    }

    pub(super) fn display_id(&self) -> &str {
        if self.id.is_empty() {
            "this operating system"
        } else {
            &self.id
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum AptSourceMode {
    /// Ubuntu <= 20.04: a single `/etc/apt/sources.list`.
    FocalList,
    /// Ubuntu >= 24.04: `/etc/apt/sources.list.d/*`.
    NobleDirectory,
    None,
}

pub(super) fn apt_source_mode(codename: &str) -> AptSourceMode {
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
pub(super) enum SetupPlan {
    TouchUserSpaceApt,
    TouchNixReady,
    WritableApt,
}

pub(super) fn setup_plan(
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

pub(super) struct AptWorkspace {
    pub(super) base: PathBuf,
    pub(super) cache: PathBuf,
    pub(super) state: PathBuf,
    pub(super) dpkg: PathBuf,
    pub(super) config: PathBuf,
    pub(super) data: PathBuf,
    pub(super) temporary_links: Vec<PathBuf>,
    pub(super) owned_paths: Vec<PathBuf>,
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

    pub(super) fn apt_get(&self) -> Command {
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

    pub(super) fn apt_cache(&self) -> Command {
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
pub(super) fn downloaded_debs(archives: &Path) -> io::Result<Vec<PathBuf>> {
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

pub(super) fn ensure_curl_xz(apt: &mut AptWorkspace) -> io::Result<()> {
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

pub(super) fn cleanup_environment(layout: InstallLayout) {
    match cleanup_workspace(layout) {
        Ok(cleanup) => cleanup.run(),
        Err(error) => eprintln!("Crackle: could not resolve cleanup paths: {error}"),
    }
}

/// `crackle clean`: legacy ran `nix-manager clean` and then `pkgunset`, with no
/// `set -e`, so a failing garbage collection never blocked cleanup.
pub(super) fn clean() -> io::Result<()> {
    let layout = resolve_layout(&invoked_name())?;
    if let Err(error) = nix_manager(&[OsString::from("clean")]) {
        eprintln!("Crackle: nix clean failed: {error}");
    }
    cleanup_environment(layout);
    Ok(())
}

/// `crackle update` owns the channel-add steps; `nix-manager update` only
/// updates channels and switches, matching the legacy split.
pub(super) fn update() -> io::Result<()> {
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

pub(super) fn nix_manager(args: &[OsString]) -> io::Result<()> {
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

pub(super) fn apt_workspace(layout: InstallLayout) -> io::Result<AptWorkspace> {
    AptWorkspace::create(layout)
}

pub(super) fn setup_repositories(apt: &AptWorkspace, layout: InstallLayout) -> io::Result<()> {
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

pub(super) fn clean_apt(apt: &AptWorkspace) -> io::Result<()> {
    let status = run_command(apt.apt_get().arg("clean"))?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("apt-get clean failed"))
    }
}

pub(super) fn resolve_apt_source_paths(contents: &str, apt_config: &Path, home: &Path) -> String {
    let apt_config = apt_config.to_string_lossy();
    let home = home.to_string_lossy();
    contents
        .replace("$HOME/.config/apt", &apt_config)
        .replace("$HOME", &home)
}

pub(super) fn check_connectivity() -> io::Result<()> {
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
