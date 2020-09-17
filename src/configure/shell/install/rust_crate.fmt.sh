#!/usr/bin/env bash

set -e

# Install Rust crate.
. $HOME/.cargo/env
cargo install {flags} "{crate_name}" 2>&1
