#!/usr/bin/env bash

set -e

# Install apt package.
apt -y update
apt -qqy install {package_names}

# Hold
apt-mark hold {held_packages}
