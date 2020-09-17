#!/usr/bin/env bash

set -e

# Create SSH directory.
mkdir -m 700 $HOME/.ssh

# Update SSHD config file to disallow password authentication and other things.
cat <<EOT | tee /etc/ssh/sshd_config >/dev/null
ChallengeResponseAuthentication no
UsePAM no
PasswordAuthentication no
PrintMotd no
AcceptEnv LANG LC_*
Subsystem	sftp	/usr/lib/openssh/sftp-server
AllowUsers {username}
EOT
