set -e

# Install dependencies.
echo "{password}" | sudo -S apt install -y openssh-server ufw docker.io
