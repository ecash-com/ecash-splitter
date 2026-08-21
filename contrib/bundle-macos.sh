#!/usr/bin/env bash
# Assemble a macOS .app bundle.
#
# `cargo run` launches a bare Mach-O, which has nowhere to carry an icon or a name — macOS reads
# both from a bundle's Info.plist. So the Dock shows a generic placeholder until this runs.
#
# This is also the bundle the signing work operates on: `rcodesign sign --code-signature-flags
# runtime` expects a .app, not a loose binary. See docs/signing-and-notarization.md.
set -euo pipefail

PROFILE="${1:-debug}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/target/$PROFILE/eCash Splitter.app"

if [ "$PROFILE" = "release" ]; then
    cargo build --release -p ecash-splitter
else
    cargo build -p ecash-splitter
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/target/$PROFILE/ecash-splitter" "$APP/Contents/MacOS/ecash-splitter"
cp "$ROOT/assets/icon.icns" "$APP/Contents/Resources/icon.icns"

VERSION="$(grep -m1 '^version' "$ROOT/app/Cargo.toml" | cut -d'"' -f2)"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>                  <string>eCash Splitter</string>
    <key>CFBundleDisplayName</key>           <string>eCash Splitter</string>
    <key>CFBundleIdentifier</key>            <string>com.ecash.splitter</string>
    <key>CFBundleExecutable</key>            <string>ecash-splitter</string>
    <key>CFBundleIconFile</key>              <string>icon</string>
    <key>CFBundlePackageType</key>           <string>APPL</string>
    <key>CFBundleShortVersionString</key>    <string>$VERSION</string>
    <key>CFBundleVersion</key>               <string>$VERSION</string>
    <key>LSMinimumSystemVersion</key>        <string>11.0</string>
    <key>NSHighResolutionCapable</key>       <true/>
</dict>
</plist>
PLIST

# macOS caches icons aggressively; touching the bundle nudges it to re-read.
touch "$APP"
echo "built: $APP"
echo "run:   open \"$APP\""
