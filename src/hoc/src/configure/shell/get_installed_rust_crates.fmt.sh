#!/usr/bin/env bash

set -e

# Get the list of installed Rust crates.
. $HOME/.cargo/env
cargo install --list | grep '.*:$' | sed -rn 's/^([^ ]*).*$/\1/p'
