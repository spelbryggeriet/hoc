#!/usr/bin/env bash

set -e

# Get the list of installed apt packages.
apt list --installed | sed -rn 's/^([^\/]*).*$/\1/p'
