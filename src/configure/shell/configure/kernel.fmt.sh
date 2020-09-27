#!/usr/bin/env bash

set -e

# Append the cgroups and swap options to the kernel command line.
echo '{content}' >/boot/cmdline.txt
