#!/usr/bin/env bash

set -e

# Update locale.
raspi-config nonint do_change_locale en_US.UTF-8 2>&1

# Update keyboard layout.
raspi-config nonint do_configure_keyboard SE 2>&1

# Update Wi-Fi country.
raspi-config nonint do_wifi_country SE 2>&1

# Update timezone.
raspi-config nonint do_change_timezone UTC 2>&1

# Update hostname.
raspi-config nonint do_hostname {hostname} 2>&1

# Uninstall unnecessary packages.
apt remove -qqy needrestart
apt autoremove -qqy
