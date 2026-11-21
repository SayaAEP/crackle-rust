# Crackle

Crackle is a Rust client for setting up Nix home-manager on Ubuntu Touch. It
keeps Nix and home-manager in the user's home directory and does not remount the
Ubuntu Touch system image or write packages to its read-only root filesystem.

## Migrating an existing Crackle installation

Migration is explicit: neither `setup` nor package `install` runs it implicitly.
Download the prebuilt ARM64 executable (no Rust toolchain is needed), then run
its migration command:

```sh
mkdir -p "$HOME/.local/bin"
curl --fail --location \
  https://github.com/SayaAEP/crackle-rust/releases/latest/download/crackle-aarch64-unknown-linux-gnu \
  --output "$HOME/.local/bin/crackle-rust"
chmod 755 "$HOME/.local/bin/crackle-rust"
"$HOME/.local/bin/crackle-rust" migrate
```

`migrate` downloads the published ARM64 binary and its `SHA256SUMS`, verifies
the binary before installation, backs up only the known Crackle-managed
executables, libraries, Python modules, and bash completion, then installs the
Rust executable and helper command names under `~/.local`. It does not change
Nix, home-manager, user configuration, or unrelated files. After reviewing the
result, run `crackle setup` when ready. To restore the most recent migration,
run `crackle rollback`; `crackle rollback ID` selects a specific saved backup.
Backups are kept under `$XDG_STATE_HOME/crackle/migrations` (or
`~/.local/state/crackle/migrations`).

## Install or update from nightly

On ARM64 Ubuntu Touch (including Ubuntu Touch 24.04 on the Redmi Note 8 Pro),
run the installer from the phone's terminal:

```sh
wget -qO- \
  https://github.com/SayaAEP/crackle-rust/releases/download/nightly/upgradefunc \
  | bash -s -- install
```

The installer asset is available after the `rust` branch's first successful
nightly workflow run.

The script checks that the device reports ARM64, downloads the prebuilt
`aarch64-unknown-linux-gnu` binary and `SHA256SUMS` from the **mutable nightly
prerelease**, verifies the checksum, installs `~/.local/bin/crackle`, and runs
`crackle setup`. It requires `wget`, `bash`, and `sha256sum`; no Rust toolchain
is needed. Re-run the same command to update to the latest nightly. Nightly
builds are for testing and may change with each push to the `rust` branch.

The installer does not migrate a legacy installation. If you need to preserve
and migrate an existing Bash-based Crackle installation, use the explicit
`crackle migrate` command described above; migration is never run implicitly.

Version-tagged releases remain separate from the mutable `nightly` prerelease.

The read-only Ubuntu Touch setup path uses user-space Apt cache, state, and
configuration directories, downloads packages with `apt-get --download-only`,
and extracts the `.deb` files without installing them system-wide. Crackle
preserves pre-existing Apt state and removes only temporary directories and tool
links it created.

## Commands

```text
crackle setup
crackle migrate
crackle rollback [ID]
crackle debug
crackle crack PACKAGE
crackle sudo BINARY
crackle sudok BINARY
crackle click PACKAGE...
crackle bump-version PACKAGE
crackle clean
crackle install PACKAGE...
crackle remove PACKAGE...
crackle update
crackle list
crackle nuke
crackle help
```

- `setup` configures Nix home-manager and installs the Rust command aliases; when
  run from `~/.local/bin/crackle`, it also updates the Nix channels and switches
  home-manager, matching the installed legacy setup workflow. It never migrates
  an older installation; use `migrate` explicitly for that.
- `migrate` explicitly replaces the legacy Crackle-managed files after a
  checksum-verified download. `rollback` restores its saved state.
- `install` and `remove` edit the home-manager package list, build the
  configuration, and activate it. Configuration is backed up before editing.
- `crack PACKAGE` downloads packages into the user's Apt workspace and extracts
  them under `~/packages/PACKAGE`.
- `click PACKAGE...` builds a Click package using the system's Click tooling.
- `remount` is an explicit standalone helper. It is never invoked by setup or
  migration.
- `nuke` removes Nix and home-manager data and is destructive; use it only when
  that is intended.

## Building from source

Rust and Cargo are needed only to build Crackle from source:

```sh
cargo build --release
cargo test
```

## Compatibility

The ARM64 Ubuntu Touch release is built in CI for
`aarch64-unknown-linux-gnu`. The command names and package/configuration paths
from the Bash version are retained. The Python package-management behavior,
standalone helper scripts, and Bash entry point are implemented by the Rust
executable; bash completion remains as a small shell integration file.
