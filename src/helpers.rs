use super::*;

pub(super) fn debug() -> io::Result<()> {
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

pub(super) fn sudo_debug() -> io::Result<()> {
    print!("{}", sudo_debug_output());
    Ok(())
}

pub(super) fn sudo_debug_output() -> &'static str {
    "SUDO CONFIG\nSUDO_CONFIG: '/root/.config'\nSUDO_BIN: '/root/.local/bin'\nSUDO_STATE: '/root/.local/state'\n"
}

pub(super) fn helper(name: &str, args: &[OsString]) -> io::Result<()> {
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

pub(super) fn fallback_triplet() -> &'static str {
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

pub(super) fn legacy_triplet_from_bash(line: &str) -> Option<String> {
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

pub(super) fn mount_option(mountpoint: &Path) -> io::Result<Option<&'static str>> {
    let mounts = fs::read_to_string("/proc/mounts")?;
    Ok(mount_option_from(&mounts, mountpoint))
}

pub(super) fn mount_option_from(mounts: &str, mountpoint: &Path) -> Option<&'static str> {
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

pub(super) fn desktop_entry(contents: &str) -> String {
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
