#!/usr/bin/env bash

set -e

# Generate a bootstrap token to authenticate nodes joining the cluster.
TOKEN=`kubeadm token generate`
VERSION=`kubelet --version | sed 's/Kubernetes //'`

# Initialize the Control Plane.
kubeadm init --token=$TOKEN --kubernetes-version=$VERSION --pod-network-cidr={cidr}
