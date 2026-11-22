# Crackle

Crackle is a Rust client for setting up Nix home-manager on Ubuntu Touch. It
keeps Nix and home-manager in the user's home directory and does not remount the
Ubuntu Touch system image or write packages to its read-only root filesystem.

## Try it beside an existing Crackle installation

The Rust binary is normally installed as `crackle`, which would collide with a
legacy Bash/Python installation. When legacy Crackle is detected (`lib/basefunc`
or a script at `bin/crackle`), the Rust installer automatically switches to a
side-by-side layout:

- installs `~/.local/bin/crackle-rust` (never replaces `bin/crackle`);
- fills in only helper names that do not already exist;
- does not touch the legacy completion, libraries, APT configuration, or data;
- uses isolated APT/data/state paths under `crackle-rust`;
- runs `setup` in **install-only** mode: Nix and home-manager are not modified
  unless `CRACKLE_SETUP_NIX=1` is set.

On a clean machine the normal `crackle` install is used. `crackle-rust migrate`
is still the explicit replacement path; `crackle-rust setup` never replaces
legacy.

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

The installer asset is available after the `rust-next` branch's first successful
nightly workflow run.

The script checks that the device reports ARM64, downloads the prebuilt
`aarch64-unknown-linux-gnu` binary and `SHA256SUMS` from the **mutable nightly
prerelease**, verifies the checksum, and installs it. On a legacy machine it
installs `~/.local/bin/crackle-rust` and leaves the existing installation
untouched; on a clean machine it installs `~/.local/bin/crackle`. It then runs
`setup` (install-only when legacy is present). It requires `wget`, `bash`,
`sha256sum`, `awk`, and `head`; no Rust toolchain is needed. Re-run the same
command to update to the latest nightly. Nightly builds are for testing and may
change with each push to the `rust-next` branch.

Overrides: `CRACKLE_INSTALL_AS` chooses the installed name, `CRACKLE_SETUP_NIX=1`
runs the full environment setup in side-by-side mode, `CRACKLE_FORCE_REPLACE=1`
allows an explicit overwrite of a legacy `crackle`, and `CRACKLE_FORCE=1` is
required for a full `nuke` while legacy Crackle is present.

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
crackle update
crackle version
crackle check-update [nightly|stable]
crackle upgrade [nightly|stable]
crackle install PACKAGE...
crackle remove PACKAGE...
crackle list
crackle clean
crackle migrate
crackle rollback [ID]
crackle debug
crackle crack PACKAGE
crackle click PACKAGE...
crackle bump-version PACKAGE
crackle sudo BINARY...
crackle sudok BINARY...
crackle nuke
crackle help
```

- `version`, `--version`, and `-V` print the embedded version, release channel,
  commit, build time, and target. Local builds identify themselves as `dev`.
- `check-update [nightly|stable]` checks release metadata only and never installs
  anything. The optional channel overrides `CRACKLE_CHANNEL`, which overrides
  the embedded channel; a local `dev` build defaults to checking nightly.
  Exit codes are `0` current, `10` update available, `11` check failed or
  identity unknown, `12` requested channel differs from the installed channel,
  and `2` invalid arguments. `CRACKLE_CHANNEL=stable` checks stable releases.
- `upgrade [nightly|stable]` updates only the Crackle executable and its owned
  helper binaries; it does not update Nix packages. It can switch channels,
  verifies the downloaded binary against that release's `SHA256SUMS`, and
  reinstalls when `CRACKLE_UPGRADE_FORCE=1` is set. Its exit codes are `0`
  updated/switched/already current, `11` fetch, verification, or install
  failure, and `2` invalid arguments. Afterward, run `crackle setup` if
  Nix/home-manager still needs configuring.
- `setup` configures Nix home-manager and installs the Rust command aliases; when
  run from `~/.local/bin/crackle`, it also updates the Nix channels and switches
  home-manager, matching the installed legacy setup workflow. It never migrates
  an older installation; use `migrate` explicitly for that.
- `migrate` explicitly replaces the legacy Crackle-managed files after a
  checksum-verified download. `rollback` restores its saved state.
- `install` and `remove` edit the home-manager package list, build the
  configuration, and activate it. Configuration is backed up before editing.
- `update` only refreshes Nix channels and activates home-manager; it does not
  update the Crackle binary. Use `check-update` to check the binary release and
  `upgrade` to install it. `clean` runs Nix garbage collection and then removes
  Crackle's own workspace, downloaded repository configuration, and dangling
  Crackle-created links. Pre-existing Apt cache/state/config directories are
  left untouched.
- `crack PACKAGE` downloads packages into the user's Apt workspace and extracts
  them under `~/packages/PACKAGE`.
- `click PACKAGE...` builds a Click package using the system's Click tooling.
- `remount` is an explicit standalone helper. It is never invoked by setup or
  migration, and it restores the original root mount mode when it finishes.
- `nuke` removes Nix and home-manager data and is destructive; use it only when
  that is intended. In an interactive terminal it asks for `[y/N]` confirmation;
  set `CRACKLE_YES=1` to skip the prompt in scripts. It continues past individual
  failures and leaves migration backups and pre-existing Apt state in place.

## Building from source

Rust and Cargo are needed only to build Crackle from source:

```sh
cargo build --release
```

The development branch is `rust-next`; its CI publishes ARM64 nightlies on
pushes and on the scheduled build. Stable releases are manually dispatched
from the GitHub Actions **Stable release** workflow with an `auto`, `patch`,
`minor`, or `major` increment. `auto` selects major for breaking changes, minor
for `feat` commits, and patch otherwise. Commit messages use Conventional
Commits (for example, `fix: preserve the existing config`). The package starts
at `0.0.0`; no release is created by a normal branch push.

## Compatibility

The ARM64 Ubuntu Touch release is built in CI for
`aarch64-unknown-linux-gnu`. The command names and package/configuration paths
from the Bash version are retained. The Python package-management behavior,
standalone helper scripts, and Bash entry point are implemented by the Rust
executable; bash completion remains as a small shell integration file.
