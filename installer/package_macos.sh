#!/usr/bin/env bash
# Build ShoutingToad.app from a compiled binary.
#   installer/package_macos.sh [path/to/shouting-toad] [output-dir]
# Defaults: target/release/shouting-toad and dist/. Drag the resulting app into /Applications.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$ROOT/target/release/shouting-toad}"
OUT="${2:-$ROOT/dist}"
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
APP="$OUT/ShoutingToad.app"

[ -f "$BIN" ] || { echo "Binary not found: $BIN (run cargo build --release first)" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/shouting-toad"
chmod +x "$APP/Contents/MacOS/shouting-toad"

# App icon from the 256 px PNG.
if command -v iconutil >/dev/null 2>&1 && command -v sips >/dev/null 2>&1; then
    ICONSET="$(mktemp -d)/AppIcon.iconset"
    mkdir -p "$ICONSET"
    for s in 16 32 64 128 256; do
        sips -z $s $s "$ROOT/assets/icon_256.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
    done
    cp "$ICONSET/icon_32x32.png" "$ICONSET/icon_16x16@2x.png"
    cp "$ICONSET/icon_64x64.png" "$ICONSET/icon_32x32@2x.png"
    cp "$ICONSET/icon_256x256.png" "$ICONSET/icon_128x128@2x.png"
    rm "$ICONSET/icon_64x64.png"
    iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
fi

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>ShoutingToad</string>
    <key>CFBundleDisplayName</key><string>ShoutingToad</string>
    <key>CFBundleExecutable</key><string>shouting-toad</string>
    <key>CFBundleIdentifier</key><string>in.ndcompany.shoutingtoad</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF

echo "Created $APP"
echo "The app is not notarised: the first time, right-click it and choose Open."
