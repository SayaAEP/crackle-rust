use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageBlock {
    package_type: String,
    start: usize,
    end: usize,
    indent: usize,
}

fn package_blocks(contents: &str) -> Vec<PackageBlock> {
    let lines: Vec<&str> = contents.lines().collect();
    let mut blocks = Vec::new();
    let mut current: Option<PackageBlock> = None;

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let detected = if line.contains(".systemPackages") {
            Some(("system".to_string(), indent))
        } else {
            line.find(".packages")
                .map(|position| (line[..position].trim().to_string(), indent))
        };
        if let Some((package_type, indent)) = detected {
            current = Some(PackageBlock {
                package_type,
                start: index + 1,
                end: lines.len(),
                indent,
            });
        } else if let Some(block) = current.as_mut()
            && line.len() - line.trim_start().len() == block.indent
            && trimmed.contains("];")
        {
            block.end = index;
            blocks.push(block.clone());
            current = None;
        }
    }
    blocks
}

fn validate_blocks(blocks: &[PackageBlock]) -> io::Result<()> {
    if blocks.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "no package blocks found in configuration file",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for block in blocks {
        if !seen.insert(&block.package_type) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "duplicate package block of type '{}' is not supported",
                    block.package_type
                ),
            ));
        }
    }
    Ok(())
}

pub fn read_packages(path: &Path, package_type: &str) -> io::Result<Vec<String>> {
    let contents = fs::read_to_string(path)?;
    let blocks = package_blocks(&contents);
    validate_blocks(&blocks)?;
    let lines: Vec<&str> = contents.lines().collect();
    let Some(block) = blocks
        .iter()
        .find(|block| block.package_type == package_type)
    else {
        return Ok(Vec::new());
    };

    Ok(lines[block.start..block.end]
        .iter()
        .map(|line| line.trim())
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with('#')
                && !line
                    .chars()
                    .any(|character| matches!(character, '(' | ')' | '[' | ']'))
        })
        .map(ToOwned::to_owned)
        .collect())
}

pub fn update_packages(
    path: &Path,
    package_type: &str,
    additions: &[String],
    removals: &[String],
    overwrite: bool,
) -> io::Result<Vec<String>> {
    let contents = fs::read_to_string(path)?;
    let blocks = package_blocks(&contents);
    validate_blocks(&blocks)?;
    let block = blocks
        .iter()
        .find(|block| block.package_type == package_type)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("package block '{package_type}' was not found"),
            )
        })?;

    let existing = if overwrite {
        Vec::new()
    } else {
        read_packages(path, package_type)?
    };
    let mut packages = Vec::with_capacity(additions.len() + existing.len());
    for package in additions {
        packages.push(if let Some(suffix) = package.strip_prefix("nixpkgs") {
            format!("pkgs{suffix}")
        } else {
            package.clone()
        });
    }
    packages.extend(existing);
    packages.retain(|package| !removals.contains(package));

    let lines: Vec<&str> = contents.lines().collect();
    let mut updated: Vec<String> = lines[..block.start]
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    updated.extend(
        packages
            .iter()
            .map(|package| format!("{}{}\n", " ".repeat(block.indent + 2), package)),
    );
    updated.extend(lines[block.end..].iter().map(|line| format!("{line}\n")));
    let mut text = updated.concat();
    if !contents.ends_with('\n') && text.ends_with('\n') {
        text.pop();
    }
    atomic_write(path, text.as_bytes())?;
    Ok(packages)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("home.nix");
    let temp = parent.join(format!(".{name}.crackle-{}.tmp", std::process::id()));
    let mut file = fs::File::create(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    if let Ok(metadata) = fs::metadata(path) {
        fs::set_permissions(&temp, metadata.permissions())?;
    }
    fs::rename(&temp, path)?;
    Ok(())
}

pub fn config_path() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    let path = home.join(".config/home-manager/home.nix");
    if path.is_file() {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("config file not found at {}", path.display()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture(contents: &str) -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("crackle-config-{id}.nix"));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn reads_and_updates_home_packages_without_touching_other_blocks() {
        let path = fixture(
            "{ pkgs, ... }: {\n  home.packages = with pkgs; [\n    git\n  ];\n  programs.bash.enable = true;\n}\n",
        );
        assert_eq!(read_packages(&path, "home").unwrap(), ["git"]);
        update_packages(&path, "home", &["nixpkgs.neovim".into()], &[], false).unwrap();
        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("    pkgs.neovim\n    git\n"));
        assert!(updated.contains("programs.bash.enable"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_duplicate_blocks_and_removes_only_requested_packages() {
        let duplicate = fixture(
            "{\n  home.packages = [\n    git\n  ];\n  home.packages = [\n    vim\n  ];\n}\n",
        );
        assert_eq!(
            read_packages(&duplicate, "home").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_file(duplicate).unwrap();

        let path = fixture(
            "{\n  home.packages = [\n    git\n    vim\n  ];\n  programs.bash.enable = true;\n}\n",
        );
        update_packages(&path, "home", &[], &["git".into()], false).unwrap();
        let updated = fs::read_to_string(&path).unwrap();
        assert!(!updated.contains("    git\n"));
        assert!(updated.contains("    vim\n"));
        assert!(updated.contains("programs.bash.enable"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reads_system_packages_using_the_legacy_block_name() {
        let path =
            fixture("{ ... }: {\n  environment.systemPackages = with pkgs; [\n    git\n  ];\n}\n");
        assert_eq!(read_packages(&path, "system").unwrap(), ["git"]);
        assert!(read_packages(&path, "home").unwrap().is_empty());
        fs::remove_file(path).unwrap();
    }
}
