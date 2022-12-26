#!/usr/bin/env python3

import os


SCRIPT_DIR = os.path.dirname(os.path.realpath(__file__))
REPO_DIR = os.path.realpath(os.path.join(SCRIPT_DIR, ".."))


def get_version():
    VERSION_NAME = "VERSION"

    version_path = os.path.join(REPO_DIR, VERSION_NAME)
    with open(version_path, "r") as f:
        content = f.read()

    return content.strip()


if __name__ == "__main__":
    print(get_version())
