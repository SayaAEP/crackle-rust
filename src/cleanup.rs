use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::command;

/// Marker written into directories Crackle creates. It lets a later `clean`
/// remove only state Crackle owns instead of the legacy `rm -r` of every APT
/// directory (which could delete a pre-existing user or system configuration).
pub(crate) const OWNED_MARKER: &str = ".crackle-owned";

pub(crate) fn mark_owned(path: &Path) {
    if let Err(error) = fs::write(path.join(OWNED_MARKER), b"crackle\n") {
        eprintln!(
            "Crackle: could not mark {} as Crackle-owned: {error}",
            path.display()
        );
    }
}

pub(crate) fn is_owned(path: &Path) -> bool {
    path.join(OWNED_MARKER).is_file()
}

pub(crate) struct Cleanup {
    pub prefix: PathBuf,
    pub pkg_path: PathBuf,
    pub apt_cache: PathBuf,
    pub apt_state: PathBuf,
    pub apt_config: PathBuf,
    pub dpkg_state: PathBuf,
    pub state_home: PathBuf,
    pub library_dir: PathBuf,
    pub command_names: &'static [&'static str],
    /// Disabled by unit tests so cleanup never invokes the real `sudo`.
    pub sudo_clean: bool,
}

impl Cleanup {
    /// Legacy `pkgunset` equivalent. Every failure is a warning: cleanup must
    /// never turn a successful command into a failure.
    pub(crate) fn run(&self) {
        for directory in [
            &self.apt_cache,
            &self.apt_state,
            &self.apt_config,
            &self.dpkg_state,
        ] {
            if is_owned(directory) {
                remove_tree(directory);
            }
        }

        if is_owned(&self.pkg_path) {
            remove_tree(&self.pkg_path);
        } else {
            // Extraction area used for curl/xz; always Crackle's own data.
            remove_tree(&self.pkg_path.join("packages"));
            remove_stale_workspaces(&self.pkg_path);
        }

        remove_tree(&self.library_dir);
        self.remove_broken_bin_links();
        self.remove_broken_home_manager_links();
        if self.sudo_clean {
            self.sudo_clean();
        }
    }

    fn remove_broken_bin_links(&self) {
        let bin = self.prefix.join("bin");
        let Ok(entries) = fs::read_dir(&bin) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_broken_symlink(&path) {
                continue;
            }
            if !self.owns_link(&path) {
                continue;
            }
            remove_file(&path);
        }
    }

    fn owns_link(&self, path: &Path) -> bool {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.command_names.contains(&name.as_str()) {
            return true;
        }
        match fs::read_link(path) {
            Ok(target) => target.starts_with(&self.pkg_path) || target.starts_with(&self.prefix),
            Err(_) => false,
        }
    }

    fn remove_broken_home_manager_links(&self) {
        let profiles = self.state_home.join("nix/profiles");
        let Ok(entries) = fs::read_dir(&profiles) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_home_manager = path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("home-manager"));
            if is_home_manager && is_broken_symlink(&path) {
                remove_file(&path);
            }
        }
    }

    fn sudo_clean(&self) {
        if std::env::var_os("CRACKLE_SKIP_SUDO_CLEAN").is_some() {
            return;
        }
        let directories = ["/root/.local/bin", "/snap/bin"];
        let existing: Vec<&str> = directories
            .into_iter()
            .filter(|directory| Path::new(directory).is_dir())
            .collect();
        if existing.is_empty() {
            return;
        }
        println!("this action requires sudo access");
        for directory in existing {
            let status = command("sudo")
                .args(["find", directory, "-xtype", "l", "-exec", "rm", "{}", "+"])
                .status();
            match status {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    eprintln!("Crackle: could not clean broken links in {directory}: {status}")
                }
                Err(error) => {
                    eprintln!("Crackle: could not clean broken links in {directory}: {error}")
                }
            }
        }
        let _ = command("sudo").arg("-k").status();
    }
}

/// Build the cleanup paths from the same resolvers the commands use.
#[allow(clippy::too_many_arguments)]
pub(crate) fn workspace(
    prefix: PathBuf,
    pkg_path: PathBuf,
    apt_cache: PathBuf,
    apt_state: PathBuf,
    apt_config: PathBuf,
    dpkg_state: PathBuf,
    state_home: PathBuf,
    library_dir: PathBuf,
    command_names: &'static [&'static str],
) -> Cleanup {
    Cleanup {
        prefix,
        pkg_path,
        apt_cache,
        apt_state,
        apt_config,
        dpkg_state,
        state_home,
        library_dir,
        command_names,
        sudo_clean: true,
    }
}

fn remove_stale_workspaces(pkg_path: &Path) {
    let Ok(entries) = fs::read_dir(pkg_path) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("crackle-apt-")
        {
            remove_tree(&entry.path());
        }
    }
}

fn is_broken_symlink(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.file_type().is_symlink() && fs::metadata(path).is_err()
}

fn remove_tree(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    let result = if metadata.file_type().is_symlink() {
        fs::remove_file(path)
    } else if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    if let Err(error) = result
        && error.kind() != io::ErrorKind::NotFound
    {
        eprintln!(
            "Crackle: could not remove {} during cleanup: {error}",
            path.display()
        );
    }
}

fn remove_file(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        eprintln!(
            "Crackle: could not remove {} during cleanup: {error}",
            path.display()
        );
    }
}
