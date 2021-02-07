#!/usr/bin/env bash

set -e

# Enable net.bridge.bridge-nf-call-iptables and -iptables6.
cat >/etc/sysctl.d/k8s.conf <<EOT
net.bridge.bridge-nf-call-ip6tables = 1
net.bridge.bridge-nf-call-iptables = 1
EOT
sysctl --system
