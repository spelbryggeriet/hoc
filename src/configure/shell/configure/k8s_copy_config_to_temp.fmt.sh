#!/usr/bin/env bash

set -e

# Copy Kubernetes config file to a temporary location.
dest=`tempfile`
cp /etc/kubernetes/admin.conf $dest
chmod +r $dest
echo $dest
