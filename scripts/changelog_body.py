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


def get_changelog_body(version):
    manifest_path = os.path.join(REPO_DIR, "CHANGELOG.md")
    with open(manifest_path, "r") as f:
        content = f.read()

    version_line = f"## [{version}]"
    split_content = content.split(version_line)

    if len(split_content) < 2:
        error("version changelog section found")
    if len(split_content) > 2:
        error("multiple version changelog sections found")

    body = split_content[1].split("## [", 1)
    body_without_title_suffix = body[0].split("\n", 1)

    if len(body_without_title_suffix) == 1:
        error("invalid changelog format")

    return body_without_title_suffix[1].strip()


if __name__ == "__main__":
    if len(sys.argv) < 2:
        error("version argument missing")

    print(get_changelog_body(sys.argv[1]))
