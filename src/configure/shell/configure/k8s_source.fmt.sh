#!/usr/bin/env bash

set -e

# Add the packages.cloud.google.com apt key.
curl -s https://packages.cloud.google.com/apt/doc/apt-key.gpg | apt-key add -

# Add the Kubernetes repository.
cat >/etc/apt/sources.list.d/kubernetes.list <<EOT
deb https://apt.kubernetes.io/ kubernetes-xenial main
EOT
