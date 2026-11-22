use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CommandGroup {
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

pub(super) struct CommandSpec {
    pub(super) name: &'static str,
    pub(super) usage: &'static str,
    pub(super) summary: &'static str,
    pub(super) group: CommandGroup,
}

/// One table for help, completion, and tests so they cannot drift.
pub(super) const COMMANDS: &[CommandSpec] = &[
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

pub(super) fn dispatch() -> io::Result<()> {
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

pub(super) fn help_text() -> String {
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

pub(super) fn show_help() -> io::Result<()> {
    print!("{}", help_text());
    Ok(())
}
