#!/usr/bin/env python3
import argparse
import pathlib
import re
import subprocess
import sys


HEADER = re.compile(r"^(?P<type>[a-z][a-z0-9-]*)(?:\([^)]+\))?(?P<breaking>!)?: .+$")
VERSION = re.compile(r"^v?(?P<major>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)$")


def parse_commit(message):
    subject = message.splitlines()[0] if message.splitlines() else ""
    match = HEADER.fullmatch(subject)
    if not match:
        raise ValueError(f"not a Conventional Commit: {subject or '<empty message>'}")
    breaking = bool(match.group("breaking")) or any(
        line.startswith(("BREAKING CHANGE:", "BREAKING-CHANGE:"))
        for line in message.splitlines()[1:]
    )
    return match.group("type"), subject, breaking


def bump_version(current, requested, messages):
    match = VERSION.fullmatch(current)
    if not match:
        raise ValueError(f"Cargo version must be SemVer X.Y.Z, got {current!r}")
    parsed = [int(match.group(name)) for name in ("major", "minor", "patch")]
    commits = [parse_commit(message) for message in messages]
    if not commits:
        raise ValueError("no commits found since the previous stable release")

    level = requested
    if requested == "auto":
        if any(breaking for _, _, breaking in commits):
            level = "major"
        elif any(kind == "feat" for kind, _, _ in commits):
            level = "minor"
        else:
            level = "patch"

    major, minor, patch = parsed
    if level == "major":
        major, minor, patch = major + 1, 0, 0
    elif level == "minor":
        major, minor, patch = major, minor + 1, 0
    elif level == "patch":
        major, minor, patch = major, minor, patch + 1
    else:
        raise ValueError(f"unsupported bump level: {requested}")
    return f"{major}.{minor}.{patch}", commits


def git(*args):
    return subprocess.check_output(["git", *args], text=True).strip()


def release_range():
    tags = git("tag", "--list", "v*").splitlines()
    versions = []
    for tag in tags:
        match = VERSION.fullmatch(tag)
        if match:
            value = tuple(int(match.group(name)) for name in ("major", "minor", "patch"))
            versions.append((value, tag))
    if versions:
        return f"{max(versions)[1]}..HEAD"
    return "HEAD"


def messages_in(revision_range):
    commits = git("log", "--no-merges", "--format=%H", "-z", revision_range).split("\0")
    messages = []
    for commit in filter(None, commits):
        body = git("show", "-s", "--format=%B", commit)
        messages.append((commit, body))
    return messages


def validate_range(revision_range):
    messages = messages_in(revision_range)
    errors = []
    for commit, message in messages:
        try:
            parse_commit(message)
        except ValueError as error:
            errors.append(f"{commit[:12]}: {error}")
    if errors:
        raise ValueError("invalid commit messages:\n" + "\n".join(errors))
    return messages


def release_notes(version, commits):
    groups = {"Features": [], "Fixes": [], "Other changes": []}
    for commit, (kind, subject, breaking) in commits:
        if kind == "feat":
            group = "Features"
        elif kind == "fix":
            group = "Fixes"
        else:
            group = "Other changes"
        suffix = " (breaking)" if breaking else ""
        groups[group].append(f"- {subject[subject.index(':') + 2:]}{suffix} ({commit[:7]})")

    lines = [f"# v{version}", ""]
    for title, entries in groups.items():
        if entries:
            lines.extend((f"## {title}", "", *entries, ""))
    return "\n".join(lines).rstrip() + "\n"


def cargo_version():
    text = pathlib.Path("Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'(?m)^version = "([^"]+)"$', text)
    if not match:
        raise ValueError("could not find package version in Cargo.toml")
    return match.group(1)


def main():
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    validate = subparsers.add_parser("validate")
    validate.add_argument("--base-ref", required=True)
    validate.add_argument("--head-ref", default="HEAD")
    prepare = subparsers.add_parser("prepare")
    prepare.add_argument("--bump", choices=("auto", "patch", "minor", "major"), required=True)
    args = parser.parse_args()

    try:
        if args.command == "validate":
            validate_range(f"{args.base_ref}..{args.head_ref}")
            print("All commit messages follow Conventional Commits.")
            return

        revision_range = release_range()
        messages = validate_range(revision_range)
        version, commits = bump_version(
            cargo_version(), args.bump, [message for _, message in messages]
        )
        pathlib.Path("release-notes.md").write_text(
            release_notes(version, list(zip((sha for sha, _ in messages), commits))),
            encoding="utf-8",
        )
        print(version)
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
