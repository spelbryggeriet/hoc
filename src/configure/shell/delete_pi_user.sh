set -e

# Kill all processes owned by pi user.
echo "{password}" | sudo -S pkill -u pi

# Delete pi user.
echo "{password}" | sudo -S deluser --remove-home pi
