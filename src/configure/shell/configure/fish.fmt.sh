#!/usr/bin/env bash

set -e

# Create Fish profile.
mkdir -p $HOME/.config/fish
cat >$HOME/.config/fish/config.fish <<EOT
# Source cargo.
. $HOME/.cargo/env

# Define some aliases.
alias fd=fdfind

# Initiate Starship prompt.
eval (starship init fish)
EOT

# Create Starship config.
cat >$HOME/.config/starship.toml <<EOT
[character]
style_success = "bold purple"
EOT

# Change default shell.
chsh -s `which fish`
