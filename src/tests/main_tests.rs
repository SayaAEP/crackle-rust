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

const TARGET_OS_RELEASE: &str = "PRETTY_NAME=\"Ubuntu 24.04.5 LTS\"
NAME=\"Ubuntu\"
VERSION_ID=\"24.04\"
VERSION=\"24.04.5 LTS (Noble Numbat)\"
VERSION_CODENAME=noble
ID=ubuntu
ID_LIKE=debian
HOME_URL=\"https://www.ubuntu.com/\"
UBUNTU_CODENAME=noble
VARIANT=Touch
VARIANT_ID=touch
";

#[test]
fn detects_the_confirmed_ubuntu_touch_release() {
    let release = OsRelease::parse(TARGET_OS_RELEASE);
    assert!(release.is_ubuntu());
    assert!(release.is_touch());
    assert_eq!(release.codename(), Some("noble"));
    assert_eq!(
        apt_source_mode(release.codename().unwrap()),
        AptSourceMode::NobleDirectory
    );
}

#[test]
fn detects_quoted_touch_and_ubuntu_touch_ids() {
    let quoted = OsRelease::parse("ID=ubuntu\nVARIANT=\"Touch\"\nVERSION_CODENAME=focal\n");
    assert!(quoted.is_ubuntu());
    assert!(quoted.is_touch());
    assert_eq!(quoted.codename(), Some("focal"));

    let id = OsRelease::parse("ID=ubuntu-touch\nNAME=\"Ubuntu Touch\"\n");
    assert!(id.is_ubuntu());
    assert!(id.is_touch());
}

#[test]
fn does_not_treat_debian_as_ubuntu_or_touch() {
    let release = OsRelease::parse("ID=debian\nID_LIKE=debian\nNAME=\"Debian GNU/Linux\"\n");
    assert!(!release.is_ubuntu());
    assert!(!release.is_touch());
    assert_eq!(release.display_id(), "debian");
}

#[test]
fn parses_click_candidate_versions() {
    assert_eq!(parse_candidate_version("1:2.3~rc1+deb1"), "2.3");
    assert_eq!(parse_candidate_version("3.4.5"), "3.4.5");
    assert_eq!(parse_candidate_version("0:9.9"), "9.9");
}

#[test]
fn reads_and_replaces_manifest_version_without_touching_the_key() {
    let text = "{\n  \"name\": \"demo\",\n  \"version\": \"1.2.3\",\n  \"x\": 1\n}\n";
    assert_eq!(manifest_version(text).as_deref(), Some("1.2.3"));
    let updated = replace_manifest_version(text, "1.2.3", "2.0.0").unwrap();
    assert!(updated.contains("\"version\": \"2.0.0\","));
    assert!(updated.contains("\"name\": \"demo\""));
    assert!(!updated.contains("1.2.3"));
}

#[test]
fn derives_the_intended_triplet_from_bash_version_output() {
    assert_eq!(
        legacy_triplet_from_bash("GNU bash, version 5.2.21(1)-release (aarch64-unknown-linux-gnu)")
            .as_deref(),
        Some("aarch64-linux-gnu")
    );
    assert_eq!(
        legacy_triplet_from_bash("GNU bash, version 5.1.16(1)-release (x86_64-pc-linux-gnu)")
            .as_deref(),
        Some("x86_64-linux-gnu")
    );
}

#[test]
fn help_and_completion_share_one_command_list() {
    let help = help_text();
    for command in COMMANDS {
        assert!(
            help.contains(command.usage),
            "help is missing {}",
            command.usage
        );
    }
    assert!(help.contains("Setup and updates"));
    assert!(help.contains("Ubuntu Touch"));
    assert!(help.contains("`check-update` only reports"));

    let mut unique: Vec<_> = COMMANDS.iter().map(|command| command.name).collect();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), COMMANDS.len(), "duplicate subcommand");
}

fn release_metadata(channel: &str, version: &str, tag: &str, commit: &str) -> ReleaseMetadata {
    ReleaseMetadata {
        channel: channel.to_owned(),
        version: version.to_owned(),
        tag: tag.to_owned(),
        commit: commit.to_owned(),
        built: "2026-11-23T02:23:00Z".to_owned(),
    }
}

#[test]
fn release_metadata_parser_requires_complete_valid_bounded_metadata() {
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let valid = format!(
        "channel=nightly\nversion=nightly-0123456\ntag=nightly\ncommit={commit}\nbuilt=2026-11-23T02:23:00Z\n"
    );
    let parsed = parse_release_metadata(&valid).unwrap();
    assert_eq!(parsed.channel, "nightly");
    assert_eq!(parsed.commit, commit);
    assert!(parse_release_metadata("channel=nightly\nversion=nightly\n").is_none());
    assert!(parse_release_metadata(&valid.replace(commit, "not-a-commit")).is_none());
    assert!(parse_release_metadata(&valid.replace("channel=nightly", "channel=dev")).is_none());
    assert!(
        parse_release_metadata(&format!(
            "{valid}extra=1\nextra2=2\nextra3=3\nextra4=4\nextra5=5\n"
        ))
        .is_none()
    );
    assert!(parse_release_metadata(&format!("{valid}note={}\n", "x".repeat(4096))).is_none());
    assert!(parse_release_metadata(&format!("{valid}channel=stable\n")).is_none());

    let stable = valid
        .replace("channel=nightly", "channel=stable")
        .replace("version=nightly-0123456", "version=v0.7")
        .replace("tag=nightly", "tag=v0.7");
    assert_eq!(parse_release_metadata(&stable).unwrap().version, "v0.7");
    assert!(parse_release_metadata(&stable.replace("version=v0.7", "version=0..7")).is_none());
}

#[test]
fn version_comparison_handles_variable_segments_and_suffixes() {
    assert_eq!(
        compare_versions(
            &parse_version_segments("0.7").unwrap(),
            &parse_version_segments("0.7.0").unwrap()
        ),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        compare_versions(
            &parse_version_segments("0.5.4").unwrap(),
            &parse_version_segments("0.5.4.1").unwrap()
        ),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        parse_version_segments("v1.2.3-rc1+build.4").unwrap(),
        vec![1, 2, 3]
    );
    assert!(parse_version_segments("1..2").is_none());
}

#[test]
fn update_decision_uses_channel_commit_and_stable_version_identity() {
    let same_commit = release_metadata(
        "nightly",
        "nightly-0123456",
        "nightly",
        "0123456789abcdef0123456789abcdef01234567",
    );
    assert_eq!(
        decide_update(
            Channel::Nightly,
            "nightly",
            "0.6.0",
            &same_commit.commit,
            &same_commit
        ),
        UpdateDecision::Current
    );
    let newer_nightly = release_metadata(
        "nightly",
        "nightly-fedcba9",
        "nightly",
        "fedcba9876543210fedcba9876543210fedcba98",
    );
    assert_eq!(
        decide_update(
            Channel::Nightly,
            "nightly",
            "0.6.0",
            &same_commit.commit,
            &newer_nightly
        ),
        UpdateDecision::UpdateAvailable
    );
    assert_eq!(
        decide_update(
            Channel::Nightly,
            "nightly",
            "0.6.0",
            "unknown",
            &same_commit
        ),
        UpdateDecision::Unknown
    );
    assert_eq!(
        decide_update(Channel::Stable, "nightly", "0.6.0", "commit", &same_commit),
        UpdateDecision::ChannelMismatch
    );

    let stable = release_metadata("stable", "0.5.4.1", "v0.5.4.1", &same_commit.commit);
    assert_eq!(
        decide_update(Channel::Stable, "stable", "0.5.4", "commit", &stable),
        UpdateDecision::UpdateAvailable
    );
    assert_eq!(
        decide_update(Channel::Stable, "stable", "0.5.4.1", "commit", &stable),
        UpdateDecision::Current
    );
    assert_eq!(
        decide_update(Channel::Stable, "stable", "0.5.4.1", "unknown", &stable),
        UpdateDecision::Unknown
    );
}

#[test]
fn upgrade_replaces_standard_commands_and_completion_atomically() {
    let root = layout_temp_dir("upgrade-standard");
    let prefix = root.join("prefix");
    let executable = root.join("new-binary");
    let completion = root.join("data/bash-completion/completions/crackle");
    fs::write(&executable, b"new executable").unwrap();
    fs::create_dir_all(prefix.join("bin")).unwrap();
    for name in COMMAND_NAMES {
        fs::write(prefix.join("bin").join(name), b"old executable").unwrap();
    }
    fs::create_dir_all(completion.parent().unwrap()).unwrap();
    fs::write(&completion, b"old completion").unwrap();

    upgrade_binaries_at(
        InstallLayout::Standard,
        &prefix,
        &executable,
        Some(&completion),
    )
    .unwrap();

    for name in COMMAND_NAMES {
        assert_eq!(
            fs::read(prefix.join("bin").join(name)).unwrap(),
            b"new executable",
            "stale command {name}"
        );
    }
    assert!(
        fs::read_to_string(completion)
            .unwrap()
            .contains("complete -F _crackle crackle")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn side_by_side_upgrade_replaces_only_rust_owned_helpers() {
    let root = layout_temp_dir("upgrade-side-by-side");
    let prefix = root.join("prefix");
    let executable = root.join("new-binary");
    fs::write(&executable, b"new executable").unwrap();
    fs::create_dir_all(prefix.join("bin")).unwrap();
    fs::create_dir_all(prefix.join("lib")).unwrap();
    fs::write(prefix.join("bin/crackle"), b"legacy crackle script").unwrap();
    fs::write(prefix.join("bin/desktop_trunc"), b"#!/usr/bin/awk -f\n").unwrap();
    fs::write(prefix.join("bin/arch_triplet"), b"\x7fELFold executable").unwrap();
    fs::write(prefix.join("lib/basefunc"), b"legacy library").unwrap();
    let completion = root.join("data/bash-completion/completions/crackle");
    fs::create_dir_all(completion.parent().unwrap()).unwrap();
    fs::write(&completion, b"legacy completion").unwrap();

    upgrade_binaries_at(InstallLayout::SideBySide, &prefix, &executable, None).unwrap();

    assert_eq!(
        fs::read(prefix.join("bin/crackle")).unwrap(),
        b"legacy crackle script"
    );
    assert_eq!(
        fs::read(prefix.join("bin/desktop_trunc")).unwrap(),
        b"#!/usr/bin/awk -f\n"
    );
    assert_eq!(
        fs::read(prefix.join("bin/arch_triplet")).unwrap(),
        b"new executable"
    );
    assert_eq!(
        fs::read(prefix.join("bin/crackle-rust")).unwrap(),
        b"new executable"
    );
    assert_eq!(
        fs::read(prefix.join("bin/remount")).unwrap(),
        b"new executable"
    );
    assert_eq!(
        fs::read(prefix.join("lib/basefunc")).unwrap(),
        b"legacy library"
    );
    assert_eq!(fs::read(completion).unwrap(), b"legacy completion");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn click_manifest_prep_matches_legacy_layout() {
    let root = env::temp_dir().join(format!(
        "crackle-click-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let build = root.join("build");
    let install = root.join("install");
    let click_bin = root.join("lib/bin");
    fs::create_dir_all(root.join("pkg.d")).unwrap();
    fs::create_dir_all(&build).unwrap();
    fs::write(root.join("pkg.d/helper.sh"), "echo helper").unwrap();
    fs::write(root.join("apparmor.json"), "{}").unwrap();
    fs::write(root.join("manifest.json"), "{\"name\": \"@pkg@\"}\n").unwrap();
    fs::write(build.join("pkg.sh"), "#!/bin/sh\n").unwrap();

    prepare_click_manifest(
        &root,
        &build,
        &install,
        &click_bin,
        &[OsString::from("demo")],
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(install.join("manifest.json")).unwrap(),
        "{\"name\": \"demo\"}\n"
    );
    assert!(install.join("apparmor.json").is_file());
    assert!(build.join("helper.sh").is_file());
    assert!(click_bin.join("demo.sh").is_file());
    fs::remove_dir_all(&root).unwrap();

    // With a template, legacy skips prep entirely and leaves the tree alone.
    let root = env::temp_dir().join(format!(
        "crackle-click-template-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let build = root.join("build");
    let install = root.join("install");
    let click_bin = root.join("lib/bin");
    fs::create_dir_all(&build).unwrap();
    fs::create_dir_all(&install).unwrap();
    fs::write(root.join("manifest.json.in"), "{}").unwrap();
    fs::write(install.join("sentinel"), "keep").unwrap();
    prepare_click_manifest(
        &root,
        &build,
        &install,
        &click_bin,
        &[OsString::from("demo")],
    )
    .unwrap();
    assert!(install.join("sentinel").is_file());
    assert!(!install.join("manifest.json").exists());
    fs::remove_dir_all(&root).unwrap();
}

fn layout_temp_dir(name: &str) -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = env::temp_dir().join(format!("crackle-layout-{name}-{id}"));
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn detects_legacy_install_markers() {
    let root = layout_temp_dir("legacy");
    let prefix = root.join("prefix");
    assert!(!legacy_install_present(&prefix));

    fs::create_dir_all(prefix.join("lib")).unwrap();
    fs::write(prefix.join("lib/basefunc"), "legacy").unwrap();
    assert!(legacy_install_present(&prefix));

    fs::remove_file(prefix.join("lib/basefunc")).unwrap();
    fs::create_dir_all(prefix.join("bin")).unwrap();
    fs::write(prefix.join("bin/crackle"), "#!/usr/bin/env bash\n").unwrap();
    assert!(legacy_install_present(&prefix));

    fs::write(prefix.join("bin/crackle"), "\x7fELFrust").unwrap();
    assert!(!legacy_install_present(&prefix));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn layout_resolution_prefers_override_then_legacy_then_name() {
    assert_eq!(
        resolve_layout_from("crackle", false, None),
        InstallLayout::Standard
    );
    assert_eq!(
        resolve_layout_from("crackle-rust", false, None),
        InstallLayout::SideBySide
    );
    assert_eq!(
        resolve_layout_from("crackle", true, None),
        InstallLayout::SideBySide
    );
    assert_eq!(
        resolve_layout_from("crackle", true, Some("standard")),
        InstallLayout::Standard
    );
    assert_eq!(
        resolve_layout_from("crackle", false, Some("side-by-side")),
        InstallLayout::SideBySide
    );
    assert_eq!(InstallLayout::SideBySide.main_name(), "crackle-rust");
    assert_eq!(
        library_dir(InstallLayout::SideBySide, Path::new("prefix")),
        Path::new("prefix/lib/crackle-rust")
    );
}

#[test]
fn side_by_side_install_preserves_legacy_files_and_fills_missing_helpers() {
    let root = layout_temp_dir("install");
    let prefix = root.join("prefix");
    let executable = root.join("rust-binary");
    fs::write(&executable, b"rust executable").unwrap();
    fs::create_dir_all(prefix.join("bin")).unwrap();
    fs::create_dir_all(prefix.join("lib")).unwrap();
    fs::write(prefix.join("bin/crackle"), "#!/usr/bin/env bash\n").unwrap();
    fs::write(prefix.join("bin/desktop_trunc"), "#!/usr/bin/awk -f\n").unwrap();
    fs::write(prefix.join("lib/basefunc"), "legacy\n").unwrap();

    install_binaries_at(InstallLayout::SideBySide, &prefix, &executable, None).unwrap();

    assert_eq!(
        fs::read_to_string(prefix.join("bin/crackle")).unwrap(),
        "#!/usr/bin/env bash\n"
    );
    assert_eq!(
        fs::read_to_string(prefix.join("bin/desktop_trunc")).unwrap(),
        "#!/usr/bin/awk -f\n"
    );
    assert_eq!(
        fs::read_to_string(prefix.join("bin/crackle-rust")).unwrap(),
        "rust executable"
    );
    assert_eq!(
        fs::read_to_string(prefix.join("bin/rorw")).unwrap(),
        "rust executable"
    );
    assert!(!root.join("completions").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn standard_install_creates_all_names_and_completion() {
    let root = layout_temp_dir("standard");
    let prefix = root.join("prefix");
    let executable = root.join("rust-binary");
    let completion = root.join("completions/crackle");
    fs::write(&executable, b"rust executable").unwrap();

    install_binaries_at(
        InstallLayout::Standard,
        &prefix,
        &executable,
        Some(&completion),
    )
    .unwrap();

    for name in COMMAND_NAMES {
        assert!(prefix.join("bin").join(name).is_file(), "missing {name}");
    }
    assert!(completion.is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn nuke_guard_protects_legacy_only_in_side_by_side_mode() {
    assert!(nuke_allowed(InstallLayout::SideBySide, true, false).is_err());
    assert!(nuke_allowed(InstallLayout::SideBySide, true, true).is_ok());
    assert!(nuke_allowed(InstallLayout::SideBySide, false, false).is_ok());
    assert!(nuke_allowed(InstallLayout::Standard, true, false).is_ok());
}

#[test]
fn downloaded_debs_tolerates_a_missing_archive_directory() {
    let root = layout_temp_dir("debs");
    assert!(downloaded_debs(&root.join("missing")).unwrap().is_empty());

    let archives = root.join("archives");
    fs::create_dir_all(&archives).unwrap();
    fs::write(archives.join("curl_1_arm64.deb"), b"deb").unwrap();
    fs::write(archives.join("index.lz4"), b"x").unwrap();
    let debs = downloaded_debs(&archives).unwrap();
    assert_eq!(debs.len(), 1);
    assert_eq!(debs[0].extension().unwrap(), "deb");
    fs::remove_dir_all(root).unwrap();
}
