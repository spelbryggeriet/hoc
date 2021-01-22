#!/usr/bin/env bash

set -e

# Create SSH directory.
mkdir -m 700 $HOME/.ssh
chown lidin:lidin $HOME/.ssh

# Update SSHD config file to disallow password authentication and other things.
cat >/etc/ssh/sshd_config <<EOT
Include /etc/ssh/sshd_config.d/*.conf
ChallengeResponseAuthentication no
UsePAM no
PasswordAuthentication no
PrintMotd no
AcceptEnv LANG LC_*
Subsystem sftp /usr/lib/openssh/sftp-server
EOT

cat >/etc/ssh/sshd_config.d/010_{username}-allousers <<EOT
AllowUsers {username}
EOT

sudo systemctl restart ssh
