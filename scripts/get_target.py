#!/usr/bin/env python3

from get_version import get_version


def get_target():
    version = get_version()
    return f"hoc_macos-x86_64_v{version}"


if __name__ == "__main__":
    print(get_target())

