#!/usr/bin/env bash

set -e

# Create or replace the contents of '/etc/docker/daemon.json' to enable the systemd cgroup driver.
cat >/etc/docker/daemon.json <<EOF
{{
  "exec-opts": ["native.cgroupdriver=systemd"],
  "log-driver": "json-file",
  "log-opts": {{
    "max-size": "100m"
  }},
  "storage-driver": "overlay2"
}}
EOF
