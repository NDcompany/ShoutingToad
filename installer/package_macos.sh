#!/usr/bin/env bash
# ShoutingToad macOS Packaging Script
set -e

APP_NAME="ShoutingToad.app"
CONTENTS="$APP_NAME/Contents"
MACOS="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"

echo "Creating macOS Application Bundle for ShoutingToad (N&D Co.)..."

mkdir -p "$MACOS"
mkdir -p "$RESOURCES"

cat << 'EOF' > "$CONTENTS/Info.plist"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>shouting-toad</string>
    <key>CFBundleIdentifier</key>
    <string>in.ndcompany.shoutingtoad</string>
    <key>CFBundleName</key>
    <string>ShoutingToad</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0.0</string>
    <key>CFBundleVersion</key>
    <string>1.0.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

cp "../target/release/shouting-toad" "$MACOS/shouting-toad" 2>/dev/null || cp "../target/debug/shouting-toad" "$MACOS/shouting-toad"
chmod +x "$MACOS/shouting-toad"

echo "✓ Created $APP_NAME bundle."
echo "Drag $APP_NAME to /Applications or package into DMG with hdiutil."
