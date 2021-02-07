#!/usr/bin/env bash

set -e

# Add new user.
cat <<EOT | adduser {username}
{password}
{password}
EOT


# Add user to relevant groups.
# usermod -a -G adm,cdrom,sudo,dip,plugdev,lxd lidin
usermod -a -G adm,dialout,cdrom,sudo,audio,video,plugdev,games,users,input,netdev,gpio,i2c,spi {username}

# Require sudo to prompt for password.
# cat >/etc/sudoers.d/010_lidin-passwd <<EOT
# {username} ALL=(ALL) PASSWD: ALL
# EOT
cat >/etc/sudoers.d/010_pi-nopasswd <<EOT
{username} ALL=(ALL) PASSWD: ALL
EOT
