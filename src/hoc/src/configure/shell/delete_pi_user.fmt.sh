#!/usr/bin/env bash

set -e

# Kill all processes owned by pi user.
pkill -u pi

# Delete pi user.
deluser --remove-home pi
