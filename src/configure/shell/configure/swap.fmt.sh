#!/usr/bin/env bash

set -e

# Uninstall any swapfiles and disable it at boot.
dphys-swapfile swapoff
dphys-swapfile uninstall

# Update the swapfile config.
cat >/etc/dphys-swapfile <<EOT
{content}
EOT
