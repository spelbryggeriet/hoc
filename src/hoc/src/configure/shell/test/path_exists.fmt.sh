#!/usr/bin/env bash

set -e

# Check if path exists.
if [ -e "{path}" ]
then
    echo true
    exit
fi

echo false
