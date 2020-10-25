#!/usr/bin/env bash

set -e

# Get a list of URLs over the available armhf Debian packages from GitHub.
curl -s https://api.github.com/repos/{repository}/releases/latest \
    | sed -rn 's/"browser_download_url": "(.*armhf.*\.deb)"/\1/p'