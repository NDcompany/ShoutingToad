#!/usr/bin/env bash
# Remove ShoutingToad installed by install_linux.sh (run with the same user, or root for a system install).
set -euo pipefail

if [ "$(id -u)" -eq 0 ]; then
    rm -f /usr/local/bin/shouting-toad /usr/share/applications/shouting-toad.desktop /usr/share/icons/hicolor/256x256/apps/shouting-toad.png
else
    rm -f "$HOME/.local/bin/shouting-toad" "$HOME/.local/share/applications/shouting-toad.desktop" "$HOME/.local/share/icons/hicolor/256x256/apps/shouting-toad.png"
fi
echo "ShoutingToad removed. Saved settings, if any, are in ~/.local/share/ShoutingToad."
