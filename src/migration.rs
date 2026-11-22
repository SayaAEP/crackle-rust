use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const ASSET: &str = "crackle-aarch64-unknown-linux-gnu";
const RELEASE_BASE: &str = "https://github.com/SayaAEP/crackle-rust/releases/latest/download";

/// Managed files live under two roots: the install prefix (`~/.local`) and the
/// XDG data directory (`bash-completion/completions`, which is not under the
/// prefix when `PKG_PREFIX` is overridden).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Root {
    Prefix,
    Data,
}

impl Root {
    fn label(self) -> &'static str {
        match self {
            Self::Prefix => "prefix",
            Self::Data => "data",
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label {
            "prefix" => Some(Self::Prefix),
            "data" => Some(Self::Data),
            _ => None,
        }
    }

    fn base<'a>(self, prefix: &'a Path, data_dir: &'a Path) -> &'a Path {
        match self {
            Self::Prefix => prefix,
            Self::Data => data_dir,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ManagedPath {
    root: Root,
    relative: PathBuf,
}

fn managed_paths(prefix: &Path) -> io::Result<Vec<ManagedPath>> {
    let mut paths: Vec<ManagedPath> = [
        "bin/crackle",
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
        "lib/nix_config/__init__.py",
        "lib/nix_config/config.py",
        "lib/nix_config/nix_config.py",
        "lib/nix_config/nix_interact.py",
        "lib/nix_config/openprocess.py",
    ]
    .into_iter()
    .map(|relative| ManagedPath {
        root: Root::Prefix,
        relative: PathBuf::from(relative),
    })
    .collect();
    paths.push(ManagedPath {
        root: Root::Data,
        relative: PathBuf::from("bash-completion/completions/crackle"),
    });
    collect_managed_files(&prefix.join("lib/crackle"), prefix, &mut paths)?;
    Ok(paths)
}

fn collect_managed_files(
    directory: &Path,
    prefix: &Path,
    paths: &mut Vec<ManagedPath>,
) -> io::Result<()> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            push_relative(directory, prefix, paths)?;
            return Ok(());
        }
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_managed_files(&path, prefix, paths)?;
        } else {
            push_relative(&path, prefix, paths)?;
        }
    }
    Ok(())
}

fn push_relative(path: &Path, prefix: &Path, paths: &mut Vec<ManagedPath>) -> io::Result<()> {
    let relative = path.strip_prefix(prefix).map_err(|error| {
        io::Error::other(format!(
            "could not make {} relative to {}: {error}",
            path.display(),
            prefix.display()
        ))
    })?;
    paths.push(ManagedPath {
        root: Root::Prefix,
        relative: relative.to_path_buf(),
    });
    Ok(())
}

fn release_asset(url: &str, destination: &Path) -> io::Result<()> {
    crate::download_file(url, destination)
        .map_err(|error| io::Error::other(format!("download failed: {url}: {error}")))
}

pub(crate) fn checksum(path: &Path) -> io::Result<String> {
    let output = Command::new("sha256sum").arg(path).output()?;
    if !output.status.success() {
        return Err(io::Error::other("sha256sum failed"));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty sha256sum output"))
}

fn copy_entry(source: &Path, destination: &Path) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)?;
        create_symlink(&target, destination)
    } else if metadata.is_file() {
        fs::copy(source, destination)?;
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to migrate non-file {}", source.display()),
        ))
    }
}

#[cfg(unix)]
fn create_symlink(target: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(windows)]
fn create_symlink(target: &Path, destination: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, destination)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn safe_relative(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
}

fn create_backup(
    prefix: &Path,
    data_dir: &Path,
    state_dir: &Path,
    paths: &[ManagedPath],
) -> io::Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let backup_dir = state_dir
        .join("migrations")
        .join(format!("{timestamp:039}-{}", std::process::id()));
    let files_dir = backup_dir.join("files");
    fs::create_dir_all(&files_dir)?;
    let mut manifest = fs::File::create(backup_dir.join("manifest.tsv"))?;
    for path in paths {
        if !safe_relative(&path.relative) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid managed path",
            ));
        }
        let current = path.root.base(prefix, data_dir).join(&path.relative);
        let backed_up = files_dir.join(path.root.label()).join(&path.relative);
        match fs::symlink_metadata(&current) {
            Ok(_) => {
                copy_entry(&current, &backed_up)?;
                writeln!(
                    manifest,
                    "present\t{}\t{}",
                    path.root.label(),
                    path.relative.display()
                )?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                writeln!(
                    manifest,
                    "absent\t{}\t{}",
                    path.root.label(),
                    path.relative.display()
                )?;
            }
            Err(error) => return Err(error),
        }
    }
    manifest.sync_all()?;
    Ok(backup_dir)
}

struct ManifestEntry {
    state: String,
    root: Root,
    relative: PathBuf,
}

fn parse_manifest(contents: &str) -> io::Result<Vec<ManifestEntry>> {
    let mut entries = Vec::new();
    for line in contents.lines() {
        let mut fields = line.split('\t');
        let state = fields.next().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid migration manifest")
        })?;
        let second = fields.next().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid migration manifest")
        })?;
        // Older manifests stored only `state\trelative`; treat those as prefix paths.
        let (root, relative) = match fields.next() {
            Some(relative) => (
                Root::from_label(second).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid migration root")
                })?,
                relative,
            ),
            None => (Root::Prefix, second),
        };
        let relative = PathBuf::from(relative);
        if !safe_relative(&relative) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsafe path in migration manifest",
            ));
        }
        entries.push(ManifestEntry {
            state: state.to_owned(),
            root,
            relative,
        });
    }
    Ok(entries)
}

fn restore_backup(prefix: &Path, data_dir: &Path, backup_dir: &Path) -> io::Result<()> {
    let manifest = fs::read_to_string(backup_dir.join("manifest.tsv"))?;
    let entries = parse_manifest(&manifest)?;
    for entry in entries.iter().rev() {
        let target = entry.root.base(prefix, data_dir).join(&entry.relative);
        let backup = backup_dir
            .join("files")
            .join(entry.root.label())
            .join(&entry.relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        match entry.state.as_str() {
            "present" => {
                if fs::symlink_metadata(&target).is_ok() {
                    fs::remove_file(&target)?;
                }
                copy_entry(&backup, &target)?;
            }
            "absent" => match fs::symlink_metadata(&target) {
                Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
                    fs::remove_file(&target)?;
                }
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("refusing to remove non-file {}", target.display()),
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            },
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid migration state",
                ));
            }
        }
    }
    Ok(())
}

pub fn migrate(prefix: &Path, data_dir: &Path, state_dir: &Path) -> io::Result<()> {
    if std::env::consts::ARCH != "aarch64" {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the prebuilt migration release currently targets ARM64 (aarch64)",
        ));
    }
    let cache = state_dir.join("migration-download");
    fs::create_dir_all(&cache)?;
    let binary = cache.join(ASSET);
    let sums = cache.join("SHA256SUMS");
    release_asset(&format!("{RELEASE_BASE}/{ASSET}"), &binary)?;
    release_asset(&format!("{RELEASE_BASE}/SHA256SUMS"), &sums)?;

    let expected = fs::read_to_string(&sums)?
        .lines()
        .find_map(|line| {
            let (digest, name) = line.split_once(char::is_whitespace)?;
            (name.trim_start_matches([' ', '*']) == ASSET).then(|| digest.to_owned())
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "release checksum is missing"))?;
    let actual = checksum(&binary)?;
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "downloaded ARM64 release failed SHA-256 verification",
        ));
    }

    let paths = managed_paths(prefix)?;
    let mut install_paths = paths.clone();
    for helper in [
        "bin/crackle",
        "bin/arch_triplet",
        "bin/desktop_trunc",
        "bin/_crackleopts",
        "bin/remount",
        "bin/rorw",
        "bin/nix-manager",
        "bin/osvariant",
        "bin/rwnix",
    ] {
        let path = ManagedPath {
            root: Root::Prefix,
            relative: PathBuf::from(helper),
        };
        if !install_paths.contains(&path) {
            install_paths.push(path);
        }
    }
    let backup_dir = create_backup(prefix, data_dir, state_dir, &install_paths)?;
    if let Err(error) = install_binary(prefix, data_dir, &binary, &install_paths) {
        if let Err(restore_error) = restore_backup(prefix, data_dir, &backup_dir) {
            return Err(io::Error::other(format!(
                "migration failed ({error}); rollback also failed ({restore_error}); backup is at {}",
                backup_dir.display()
            )));
        }
        return Err(error);
    }
    let _ = fs::remove_dir_all(&cache);
    println!(
        "Migration complete. Legacy managed files are backed up at {}. Run `crackle setup` when ready.",
        backup_dir.display()
    );
    if !prefix.join("bin").is_dir() {
        eprintln!(
            "Add {}/bin to PATH to use the installed Crackle commands.",
            prefix.display()
        );
    }
    Ok(())
}

fn install_binary(
    prefix: &Path,
    data_dir: &Path,
    binary: &Path,
    paths: &[ManagedPath],
) -> io::Result<()> {
    let bin_dir = prefix.join("bin");
    fs::create_dir_all(&bin_dir)?;
    for path in paths
        .iter()
        .filter(|path| path.root == Root::Prefix && path.relative.starts_with("bin"))
    {
        let destination = prefix.join(&path.relative);
        let temporary = destination.with_extension("crackle-new");
        fs::copy(binary, &temporary)?;
        make_executable(&temporary)?;
        if fs::symlink_metadata(&destination).is_ok() {
            fs::remove_file(&destination)?;
        }
        fs::rename(temporary, destination)?;
    }
    for path in paths
        .iter()
        .filter(|path| !(path.root == Root::Prefix && path.relative.starts_with("bin")))
    {
        let target = path.root.base(prefix, data_dir).join(&path.relative);
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
                fs::remove_file(target)?;
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("refusing to remove non-file {}", target.display()),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub fn rollback(
    prefix: &Path,
    data_dir: &Path,
    state_dir: &Path,
    requested: Option<&str>,
) -> io::Result<()> {
    let migrations = state_dir.join("migrations");
    let backup = if let Some(name) = requested {
        if !name.split_once('-').is_some_and(|(timestamp, pid)| {
            !timestamp.is_empty()
                && timestamp.bytes().all(|byte| byte.is_ascii_digit())
                && !pid.is_empty()
                && pid.bytes().all(|byte| byte.is_ascii_digit())
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "migration ID must be the numeric ID shown in the backup directory",
            ));
        }
        let candidate = migrations.join(name);
        if !candidate.join("manifest.tsv").is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("migration backup '{name}' was not found"),
            ));
        }
        candidate
    } else {
        fs::read_dir(&migrations)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.join("manifest.tsv").is_file())
            .max_by_key(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.split('-').next())
                    .and_then(|name| name.parse::<u128>().ok())
                    .unwrap_or_default()
            })
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "no migration backup was found")
            })?
    };
    restore_backup(prefix, data_dir, &backup)?;
    println!("Restored Crackle-managed files from {}.", backup.display());
    Ok(())
}
