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

fn raw_lines(contents: &str) -> Vec<&str> {
    if contents.is_empty() {
        Vec::new()
    } else {
        contents.split_inclusive('\n').collect()
    }
}

fn line_text(raw: &str) -> &str {
    raw.trim_end_matches(['\r', '\n'])
}

fn package_blocks(contents: &str) -> Vec<PackageBlock> {
    let lines = raw_lines(contents);
    let mut blocks = Vec::new();
    let mut current: Option<PackageBlock> = None;

    for (index, raw) in lines.iter().enumerate() {
        let line = line_text(raw);
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

// Mirrors the original `check_package_blocks()`: prints a message and
// reports "there's a problem" rather than raising an error. The caller
// is expected to just return an empty/unchanged result in that case and
// keep going, same as the Python implementation did.
fn has_block_problem(blocks: &[PackageBlock]) -> bool {
    if blocks.is_empty() {
        println!("no package blocks found in configuration file");
        return true;
    }
    let mut seen = std::collections::HashSet::new();
    for block in blocks {
        if !seen.insert(&block.package_type) {
            println!(
                "Error: Duplicate package block of type '{}' this is not supported.",
                block.package_type
            );
            return true;
        }
    }
    false
}

pub fn read_packages(path: &Path, package_type: &str) -> io::Result<Vec<String>> {
    let contents = fs::read_to_string(path)?;
    let blocks = package_blocks(&contents);
    if has_block_problem(&blocks) {
        return Ok(Vec::new());
    }
    let lines = raw_lines(&contents);
    let Some(block) = blocks
        .iter()
        .find(|block| block.package_type == package_type)
    else {
        return Ok(Vec::new());
    };

    Ok(lines[block.start..block.end]
        .iter()
        .map(|raw| line_text(raw).trim())
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
    if has_block_problem(&blocks) {
        return Ok(Vec::new());
    }
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
        packages.push(if let Some(suffix) = package.strip_prefix("nixpkgs.") {
            format!("pkgs.{suffix}")
        } else {
            package.clone()
        });
    }
    packages.extend(existing);
    packages.retain(|package| !removals.contains(package));

    // Preserve every untouched line byte-for-byte (including CRLF) and use the
    // file's dominant line ending for the rewritten block.
    let lines = raw_lines(&contents);
    let eol = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut text = String::with_capacity(contents.len());
    for raw in &lines[..block.start] {
        text.push_str(raw);
    }
    for package in &packages {
        text.push_str(&" ".repeat(block.indent + 2));
        text.push_str(package);
        text.push_str(eol);
    }
    for raw in &lines[block.end..] {
        text.push_str(raw);
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
#[path = "tests/config_tests.rs"]
mod tests;
