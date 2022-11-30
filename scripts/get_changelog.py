#!/usr/bin/env python3

from get_version import get_version
from util import error
import json
import os
import sys


SCRIPT_DIR = os.path.dirname(os.path.realpath(__file__))
REPO_DIR = os.path.realpath(os.path.join(SCRIPT_DIR, ".."))


def get_changelog_body():
    manifest_path = os.path.join(REPO_DIR, "CHANGELOG.md")
    with open(manifest_path, "r") as f:
        content = f.read()

    version = get_version()
    version_line = f"## [{version}]"
    version_split= content.split(version_line)

    if len(version_split) < 2:
        error("version changelog section found")
    if len(version_split) > 2:
        error("multiple version changelog sections found")

    title_suffix_with_body_split = version_split[1].split("## [", 1)
    body_split = title_suffix_with_body_split[0].split("\n", 1)

    if len(body_split) == 1:
        error("invalid changelog format")

    stripped_body = body_split[1].strip()
    return json.dumps(stripped_body)


if __name__ == "__main__":
    print(get_changelog_body())
