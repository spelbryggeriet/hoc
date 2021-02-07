#!/usr/bin/env bash

set -e

# Check if Rust is installed.
if [ -e $HOME/.cargo/env ]
then
    output=`. $HOME/.cargo/env 1>/dev/null 2>&1; command -v cargo`
    if [ "$output" ]
    then
        echo true
        exit
    fi
fi

echo false
