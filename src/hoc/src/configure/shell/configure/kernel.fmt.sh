#!/usr/bin/env bash

set -e

# Update the kernel command line.
cat >/boot/cmdline.txt <<EOT
{content}
EOT
