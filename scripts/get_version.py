#!/usr/bin/env python3

import os
from util import error


SCRIPT_DIR = os.path.dirname(os.path.realpath(__file__))
REPO_DIR = os.path.realpath(os.path.join(SCRIPT_DIR, ".."))


def split(content, sep, desc=None):
    parts = content.split(sep, 1)
    if len(parts) == 1:
        if desc is not None:
            error(desc)
        else:
            error(f'"{sep}" separator not found')
    return parts


def get_version(content = None):
    HEADER_KEY = "[package]"
    VERSION_KEY = "version"
    EQUALS = "="
    NEW_LINE = "\n"

    if content is None:
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

        return line.strip().strip('"')


if __name__ == "__main__":
    print(get_version())
