#!/usr/bin/env bash

set -e

# Download the Flannel YAML data and apply it
curl -sSL https://raw.githubusercontent.com/coreos/flannel/v0.12.0/Documentation/kube-flannel.yml \
    | kubectl apply -f -
