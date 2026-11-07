#!/bin/bash
# Builds a Pro Wave Mac release: a universal (Apple silicon + Intel) "Pro Wave Mac.app",
# ad-hoc signed, then
#   prowave-macos-universal.zip   the app, for the in-app updater (Help > Check for Updates…)
#   ProWave-Mac.dmg               the download: the app and a link to Applications
#   SHA256SUMS
# in OUT (default dist/macos/out). Run from anywhere in the checkout:
#   bash dist/macos/release.sh [OUT]
# Needs Xcode's command line tools; the two Rust targets are added when missing. Set
# CARGO_TARGET_DIR to build somewhere else than target/.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$(mkdir -p "${1:-$ROOT/dist/macos/out}" && cd "${1:-$ROOT/dist/macos/out}" && pwd)"
NAME="Pro Wave Mac"
BUNDLE_ID="com.yuribeats.prowave-mac"
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
cd "$ROOT"

VERSION=$(cargo pkgid -p prowave-app | sed 's/.*[#@]//')
# The oldest macOS the app runs on (LSMinimumSystemVersion below says the same).
export MACOSX_DEPLOYMENT_TARGET=11.0
# The program's panic messages name source files: the builder's home folder stays out of them.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~"
for t in "${TARGETS[@]}"; do
  rustup target list --installed | grep -qx "$t" || rustup target add "$t"
  cargo build --release -p prowave-app --target "$t"
done

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
APP="$STAGE/$NAME.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/prowave" \
  "$TARGET_DIR/aarch64-apple-darwin/release/prowave" \
  "$TARGET_DIR/x86_64-apple-darwin/release/prowave"
cp dist/macos/prowave.icns "$APP/Contents/Resources/prowave.icns"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>${NAME}</string>
  <key>CFBundleDisplayName</key><string>${NAME}</string>
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
  <key>CFBundleExecutable</key><string>prowave</string>
  <key>CFBundleIconFile</key><string>prowave</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>${NAME} records audio from your inputs.</string>
</dict>
</plist>
EOF
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
lipo -archs "$APP/Contents/MacOS/prowave"

rm -f "$OUT/prowave-macos-universal.zip" "$OUT/ProWave-Mac.dmg" "$OUT/SHA256SUMS"
# The updater unpacks the zip and takes the .app at its top (see updater.rs unpack_app).
ditto -c -k --keepParent "$APP" "$OUT/prowave-macos-universal.zip"
DMG_DIR="$STAGE/dmg"
mkdir -p "$DMG_DIR"
cp -R "$APP" "$DMG_DIR/"
ln -s /Applications "$DMG_DIR/Applications"
hdiutil create -quiet -volname "$NAME" -srcfolder "$DMG_DIR" -ov -format UDZO "$OUT/ProWave-Mac.dmg"
(cd "$OUT" && shasum -a 256 prowave-macos-universal.zip ProWave-Mac.dmg > SHA256SUMS)
echo "Pro Wave Mac $VERSION:"
ls -l "$OUT"
