#!/usr/bin/env bash

set -e

# Disable firewall.
ufw disable

# Delete all previously user-defined firewall rules.
OLD_IFS=IFS
IFS=$'\n'
for rule in `ufw show added | grep ^ufw`
do
    args=`echo "$rule" | sed 's/ufw/ufw delete/'`
    readarray -t -d '' args <<<`xargs printf '%s\n' <<<"$args"`
    ${{args[@]}}
done
IFS=OLD_IFS

# Allow to connect to port 22 on local network only.
ufw allow 22/tcp

# Enable firewall.
yes | ufw enable
