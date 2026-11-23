#!/usr/bin/env bash
# Uninstaller for ShoutingToad on Linux systems
set -e

if [ "$EUID" -ne 0 ]; then
    echo "Please execute as root: sudo ./uninstall_linux.sh"
    exit 1
fi

rm -f /usr/local/bin/shouting-toad
rm -f /usr/share/applications/shouting-toad.desktop

echo "ShoutingToad removed successfully."
