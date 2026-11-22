use super::*;
use std::io::Write;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Channel {
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

pub(super) fn version() -> io::Result<()> {
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

pub(super) struct ReleaseMetadata {
    pub(super) channel: String,
    pub(super) version: String,
    pub(super) tag: String,
    pub(super) commit: String,
    pub(super) built: String,
}

pub(super) fn parse_release_metadata(text: &str) -> Option<ReleaseMetadata> {
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

pub(super) fn parse_version_segments(version: &str) -> Option<Vec<u64>> {
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

pub(super) fn compare_versions(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
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
pub(super) enum UpdateDecision {
    Current,
    UpdateAvailable,
    ChannelMismatch,
    Unknown,
}

pub(super) fn decide_update(
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

pub(super) fn check_update(values: &[OsString]) -> io::Result<()> {
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

pub(super) fn upgrade(values: &[OsString]) -> io::Result<()> {
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

pub(super) fn upgrade_install() -> io::Result<()> {
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

pub(super) fn upgrade_binaries_at(
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
