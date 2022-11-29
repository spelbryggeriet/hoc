#!/usr/bin/env python3

import datetime
import os
import subprocess
import sys


SCRIPT_DIR = os.path.dirname(os.path.realpath(__file__))
REPO_DIR = os.path.realpath(os.path.join(SCRIPT_DIR, ".."))


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def error(msg):
    eprint("error:", msg)
    sys.exit(1)


def run(*cmd):
    output = subprocess.run(cmd, capture_output=True)
    return output.stdout.decode("utf-8"), output.stderr.decode("utf-8")


def split(content, sep, desc=None):
    parts = content.split(sep, 1)
    if len(parts) == 1:
        if desc is not None:
            error(desc)
        else:
            error(f'"{sep}" separator not found')
    return parts


def get_next_version(bump_comp_idx, current_version):
    components = current_version.split(".")

    all_digits = lambda c: all(map(str.isdigit, c))
    is_empty = lambda c: len(c) == 0
    is_int = lambda c: not is_empty(c) and all_digits(c)

    has_three_components = len(components) == 3
    all_components_are_ints = all(map(is_int, components))

    if not (has_three_components and all_components_are_ints):
        error(f'"{version}" version number invalid')

    components[bump_comp_idx] = int(components[bump_comp_idx]) + 1
    [major, minor, patch] = [*components[:bump_comp_idx+1], 0, 0, 0][:3]

    return f"{major}.{minor}.{patch}"


def update_manifest(bump_comp_idx):
    HEADER_KEY = "[package]"
    VERSION_KEY = "version"
    EQUALS = "="
    NEW_LINE = "\n"

    manifest_path = os.path.join(REPO_DIR, "Cargo.toml")
    with open(manifest_path, "r") as f:
        content = f.read()

    [parsed, content] = split(content, HEADER_KEY)
    parsed += HEADER_KEY
    while len(content) > 0:
        [line, content] = split(content, NEW_LINE)

        if len(line.strip()) == 0:
            parsed += line + NEW_LINE
            continue

        [key, line] = split(line, EQUALS, f'"{VERSION_KEY}" key not found')
        parsed += key + EQUALS

        if key.strip() != VERSION_KEY:
            parsed += line + NEW_LINE
            continue

        version = line.strip().strip('"') 
        new_version = get_next_version(bump_comp_idx, version)

        parsed += f' "{new_version}"' + NEW_LINE + content
        with open(manifest_path, "w") as f:
            f.write(parsed)

        return new_version


def update_changelog(new_version):
    HEADER_KEY = "## [Unreleased]"
    CHANGELOG_NAME = "CHANGELOG.md"

    changelog_path = os.path.join(REPO_DIR, CHANGELOG_NAME)
    last_version, _ = run("git", "-C", REPO_DIR, "rev-list", "--date-order", "--tags", "--max-count=1")
    stdout, stderr = run("git", "-C", REPO_DIR, "diff", last_version, "HEAD", "--", CHANGELOG_NAME)

    if len(stderr) > 0:
        error(stderr.strip())
    if len(stdout) == 0:
        error("no changes have been made to the changelog")

    with open(changelog_path, "r") as f:
        content = f.read()

    current_date = datetime.datetime.now(datetime.timezone.utc)
    formatted_date = current_date.strftime("%Y-%m-%d")
    replacement = f"## [{new_version}] - {formatted_date}"
    new_content = content.replace(HEADER_KEY, replacement, 1)

    if content == new_content:
        error("no unreleased version section found")
    if new_content != new_content.replace(HEADER_KEY, replacement):
        error("multiple unreleased version sections found")

    new_content = new_content.replace(replacement, f"{HEADER_KEY}\n\n{replacement}")
    with open(changelog_path, "w") as f:
        f.write(new_content)


def bump_version(bump_comp_idx):
    new_version = update_manifest(bump_comp_idx)
    update_changelog(new_version)
    return new_version


if __name__ == "__main__":
    if len(sys.argv) < 2:
        error("component argument missing")

    component = sys.argv[1]
    if component == "major":
        bump_comp_idx = 0
    elif component == "minor":
        bump_comp_idx = 1
    elif component == "patch":
        bump_comp_idx = 2
    else:
        error(f'"major", "minor" or "patch" expected')

    print(bump_version(bump_comp_idx))
