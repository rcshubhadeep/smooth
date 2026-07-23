#!/usr/bin/env bash
# Fallback: deep ad-hoc re-sign the built macOS app and rebuild its DMG.
#
# Not normally needed — `bundle.macOS.signingIdentity: "-"` in tauri.conf.json
# makes `tauri build` produce a valid ad-hoc seal already. Use this only if a
# built app ever fails `codesign --verify` again.
#
# This produces an AD-HOC signed (not notarized) app: recipients must still
# clear quarantine once with:
#   xattr -dr com.apple.quarantine /Applications/Smooth.app
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/src-tauri/target/release/bundle/macos/Smooth.app"
DMG_DIR="$ROOT/src-tauri/target/release/bundle/dmg"

[ -d "$APP" ] || { echo "App not found: $APP (run tauri build first)"; exit 1; }

echo "Re-signing (deep ad-hoc)…"
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
echo "Signature valid."

STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
OUT="$DMG_DIR/Smooth_resigned.dmg"
rm -f "$OUT"
hdiutil create -volname "Smooth" -srcfolder "$STAGE" -ov -format UDZO "$OUT" >/dev/null
rm -rf "$STAGE"
echo "Wrote $OUT"
