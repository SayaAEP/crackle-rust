use std::env;
use std::process::Command;

fn main() {
    watch_git_identity();
    for key in [
        "CRACKLE_VERSION",
        "CRACKLE_CHANNEL",
        "CRACKLE_COMMIT",
        "CRACKLE_BUILT",
        "SOURCE_DATE_EPOCH",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }

    let version = env::var("CRACKLE_VERSION")
        .or_else(|_| env::var("CARGO_PKG_VERSION"))
        .unwrap_or_else(|_| "unknown".to_owned());
    let channel = env::var("CRACKLE_CHANNEL").unwrap_or_else(|_| "dev".to_owned());
    let commit = env::var("CRACKLE_COMMIT")
        .ok()
        .or_else(git_head)
        .unwrap_or_else(|| "unknown".to_owned());
    let built = env::var("CRACKLE_BUILT")
        .ok()
        .or_else(|| env::var("SOURCE_DATE_EPOCH").ok())
        .unwrap_or_else(|| "unknown".to_owned());
    let target = env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned());

    println!("cargo:rustc-env=CRACKLE_VERSION={version}");
    println!("cargo:rustc-env=CRACKLE_CHANNEL={channel}");
    println!("cargo:rustc-env=CRACKLE_COMMIT={commit}");
    println!("cargo:rustc-env=CRACKLE_BUILT={built}");
    println!("cargo:rustc-env=CRACKLE_TARGET={target}");
}

fn watch_git_identity() {
    for args in [
        &["rev-parse", "--git-path", "HEAD"][..],
        &["rev-parse", "--git-path", "packed-refs"][..],
    ] {
        if let Some(path) = git_output(args) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(reference) = git_output(&["symbolic-ref", "--quiet", "HEAD"])
        && let Some(path) = git_output(&["rev-parse", "--git-path", &reference])
    {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn git_head() -> Option<String> {
    let commit = git_output(&["rev-parse", "HEAD"])?;
    if commit.is_empty() {
        None
    } else {
        Some(commit)
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if commit.is_empty() {
        None
    } else {
        Some(commit)
    }
}
