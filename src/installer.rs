use super::*;

pub(super) fn install_binary_as(bin: &Path, name: &str, executable: &Path) -> io::Result<()> {
    let target = bin.join(name);
    let temporary = bin.join(format!(".{name}.new"));
    fs::copy(executable, &temporary)?;
    set_executable(&temporary)?;
    fs::rename(temporary, target)
}

pub(super) fn write_completion(completion: &Path) -> io::Result<()> {
    let content = include_str!("../share/bash-completion/completions/crackle");
    atomic_write(completion, content.as_bytes())
}

pub(super) fn install_binaries(layout: InstallLayout) -> io::Result<()> {
    let executable = env::current_exe()?;
    let completion = if layout.is_side_by_side() {
        None
    } else {
        Some(completion_file()?)
    };
    install_binaries_at(layout, &prefix()?, &executable, completion.as_deref())
}

pub(super) fn install_binaries_at(
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
            println!("Installed Crackle commands in {}.", bin.display());
        }
        InstallLayout::SideBySide => {
            install_binary_as(&bin, layout.main_name(), executable)?;
            for name in COMMAND_NAMES
                .iter()
                .copied()
                .filter(|name| *name != "crackle")
            {
                let destination = bin.join(name);
                if fs::symlink_metadata(&destination).is_ok() {
                    continue;
                }
                install_binary_as(&bin, name, executable)?;
            }
            if legacy_install_present(prefix) {
                println!(
                    "Legacy Crackle detected; installed side-by-side as {}. Existing Crackle files were left untouched.",
                    bin.join(layout.main_name()).display()
                );
            } else {
                println!(
                    "Installed Crackle side-by-side as {}.",
                    bin.join(layout.main_name()).display()
                );
            }
        }
    }
    Ok(())
}
