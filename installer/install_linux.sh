#!/usr/bin/env bash
# ShoutingToad Linux Native Installer
set -e

INSTALL_BIN="/usr/local/bin/shouting-toad"
DESKTOP_ENTRY="/usr/share/applications/shouting-toad.desktop"
ICON_DIR="/usr/share/icons/hicolor/scalable/apps"

echo "Installing ShoutingToad (by N&D Co. • https://ndcompany.in)..."

if [ "$EUID" -ne 0 ]; then
    echo "Please run as root (sudo ./install_linux.sh)"
    exit 1
fi

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_BIN="$DIR/../target/release/shouting-toad"
if [ ! -f "$SOURCE_BIN" ]; then
    SOURCE_BIN="$DIR/../target/debug/shouting-toad"
fi

cp "$SOURCE_BIN" "$INSTALL_BIN"
chmod +x "$INSTALL_BIN"

cp "$DIR/shouting-toad.desktop" "$DESKTOP_ENTRY"
chmod 644 "$DESKTOP_ENTRY"

echo "✓ ShoutingToad successfully installed to $INSTALL_BIN"
echo "Launchable from application menu or via command: shouting-toad"
