#!/usr/bin/env bash
# Install ShoutingToad on Linux.
#
#   From a release archive:   ./install_linux.sh
#   From a source checkout:   cargo build --release && ./installer/install_linux.sh
#   Straight from GitHub:     curl -fsSL https://raw.githubusercontent.com/NDcompany/ShoutingToad/main/installer/install_linux.sh | bash
#
# Run as root to install system-wide (/usr/local), otherwise it installs for the current user (~/.local).
set -euo pipefail

REPO="NDcompany/ShoutingToad"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || pwd)"

if [ "$(id -u)" -eq 0 ]; then
    PREFIX="/usr/local"
    APPS="/usr/share/applications"
    ICONS="/usr/share/icons/hicolor/256x256/apps"
else
    PREFIX="$HOME/.local"
    APPS="$HOME/.local/share/applications"
    ICONS="$HOME/.local/share/icons/hicolor/256x256/apps"
fi
BIN="$PREFIX/bin"

find_file() {
    for candidate in "$@"; do
        [ -f "$candidate" ] && { echo "$candidate"; return 0; }
    done
    return 1
}

TMP=""
cleanup() { [ -n "$TMP" ] && rm -rf "$TMP"; }
trap cleanup EXIT

SRC_BIN="$(find_file "$DIR/shouting-toad" "$DIR/../target/release/shouting-toad" || true)"
if [ -z "$SRC_BIN" ]; then
    arch="$(uname -m)"
    if [ "$arch" != "x86_64" ]; then
        echo "No prebuilt binary for $arch. Build from source: cargo build --release" >&2
        exit 1
    fi
    echo "Downloading the latest ShoutingToad release…"
    TMP="$(mktemp -d)"
    curl -fsSL "https://github.com/$REPO/releases/latest/download/shouting-toad-x86_64-unknown-linux-gnu.tar.gz" | tar -xz -C "$TMP"
    SRC_BIN="$TMP/shouting-toad"
    DIR="$TMP"
fi

mkdir -p "$BIN" "$APPS" "$ICONS"
install -m 755 "$SRC_BIN" "$BIN/shouting-toad"

DESKTOP="$(find_file "$DIR/shouting-toad.desktop" "$DIR/../installer/shouting-toad.desktop" || true)"
ICON="$(find_file "$DIR/icon_256.png" "$DIR/../assets/icon_256.png" || true)"
if [ -n "$ICON" ]; then
    install -m 644 "$ICON" "$ICONS/shouting-toad.png"
fi
if [ -n "$DESKTOP" ]; then
    sed "s|^Exec=.*|Exec=$BIN/shouting-toad %f|" "$DESKTOP" > "$APPS/shouting-toad.desktop"
    chmod 644 "$APPS/shouting-toad.desktop"
fi
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS" >/dev/null 2>&1 || true

echo "ShoutingToad installed to $BIN/shouting-toad"
case ":$PATH:" in
    *":$BIN:"*) ;;
    *) echo "Add $BIN to your PATH to run 'shouting-toad' from a terminal." ;;
esac
