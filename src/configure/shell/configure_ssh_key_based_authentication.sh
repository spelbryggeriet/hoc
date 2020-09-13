set -e

# Create SSH directory.
mkdir /home/{username}/.ssh

# Update SSHD config file to disallow password authentication and other things.
cat <<EOT | sudo -S tee /etc/ssh/sshd_config >/dev/null
{password}
ChallengeResponseAuthentication no
UsePAM no
PasswordAuthentication no
PrintMotd no
AcceptEnv LANG LC_*
Subsystem	sftp	/usr/lib/openssh/sftp-server
AllowUsers {username}
EOT
