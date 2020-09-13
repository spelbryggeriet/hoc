set -e

# Update hostname.
cat <<EOT | sudo -S tee /etc/hostname >/dev/null
{password}
{hostname}
EOT

cat <<EOT | sudo -S tee /etc/hosts >/dev/null
{password}
127.0.0.1	localhost
::1		localhost ip6-localhost ip6-loopback
ff02::1		ip6-allnodes
ff02::2		ip6-allrouters

127.0.1.1		{hostname}
EOT
