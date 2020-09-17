#!/usr/bin/env bash

set -e

# Download the Debian package from GitHub.
temp_path=`mktemp`
wget "{url}" -q -O "$temp_path"

# Promote package.
dpkg -i "$temp_path"
