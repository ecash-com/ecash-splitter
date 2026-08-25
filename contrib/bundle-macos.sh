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

# Version lives once, in [workspace.package] at the repo root.
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"

# Shares contrib/Info.plist.in with the release workflow, so a local bundle and a released one
# cannot describe themselves differently.
sed -e "s/__VERSION__/$VERSION/g" "$ROOT/contrib/Info.plist.in" > "$APP/Contents/Info.plist"

# macOS caches icons aggressively; touching the bundle nudges it to re-read.
touch "$APP"
echo "built: $APP"
echo "run:   open \"$APP\""
