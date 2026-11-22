use super::*;

pub(super) fn crack(args: &[OsString]) -> io::Result<()> {
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

pub(super) fn click(args: &[OsString], bump: bool) -> io::Result<()> {
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

pub(super) fn package_with_arch(package: &OsString, arch: &str) -> OsString {
    if package.to_string_lossy().ends_with(":all") {
        package.clone()
    } else {
        OsString::from(format!("{}:{arch}", package.to_string_lossy()))
    }
}

pub(super) fn prepare_click_manifest(
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
pub(super) fn parse_candidate_version(candidate: &str) -> String {
    let stripped = candidate.split(['~', '+']).next().unwrap_or(candidate);
    stripped
        .split_once(':')
        .map(|(_, version)| version)
        .unwrap_or(stripped)
        .to_owned()
}

/// Legacy awk read field 2 of the first line containing `version`, removed
/// quotes, then dropped the trailing comma (`"version": "1.2.3",` → `1.2.3`).
pub(super) fn manifest_version(contents: &str) -> Option<String> {
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
pub(super) fn replace_manifest_version(
    contents: &str,
    current: &str,
    new_version: &str,
) -> Option<String> {
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
