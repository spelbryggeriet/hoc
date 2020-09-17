#!/usr/bin/env bash

set -e

# Setup crontab to update opnessh-server every day.
cat <<EOT | crontab -
0 3 * * * apt install -y openssh-server
15 * * * * tmpreaper 12h /tmp
EOT
