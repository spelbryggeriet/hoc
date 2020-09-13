set -e

# Add new user.
cat <<EOT | sudo adduser {username}
{password}
{password}
EOT

# Add user to relevant groups.
sudo usermod -a -G adm,dialout,cdrom,sudo,audio,video,plugdev,games,users,input,netdev,gpio,i2c,spi {username}

# Require sudo to prompt for password.
cat <<EOT | sudo tee /etc/sudoers.d/010_pi-nopasswd >/dev/null
{username} ALL=(ALL) PASSWD: ALL
EOT
