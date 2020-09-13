set -e

# Enable firewall.
(echo "{password}"; yes) | sudo -S ufw enable

# Allow to connect to port 22 on local network only.
echo "{password}" | sudo -S ufw allow 22/tcp
