set -e

# Setup crontab to update opnessh-server every day.
cat <<EOT | sudo -S crontab -
{password}
0 3 * * * apt install -y openssh-server
EOT
