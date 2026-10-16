#!/bin/sh
# Builds Vario.app for macOS: the interface (vario-gui) and the command line (vario)
# side by side in Contents/MacOS, since the hooks the interface installs run the
# `vario` command next to it. Writes dist/Vario.app and dist/Vario-macos-<arch>.zip.
#
# Usage: scripts/macos-app.sh [target]   e.g. aarch64-apple-darwin (default: this Mac)
#
# The app is signed ad hoc only (Apple Silicon refuses unsigned binaries). Without an
# Apple Developer ID and notarization, macOS warns on first launch: open it with a
# right click, Open.
set -eu

cd "$(dirname "$0")/.."
target="${1:-}"
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)

if [ -n "$target" ]; then
  cargo build --release --locked --target "$target" -p vario -p vario-gui
  bin="target/$target/release"
  arch="${target%%-*}"
else
  cargo build --release --locked -p vario -p vario-gui
  bin="target/release"
  arch=$(uname -m)
fi

app="dist/Vario.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin/vario-gui" "$bin/vario" "$app/Contents/MacOS/"
cp LICENSE "$app/Contents/Resources/"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Vario</string>
  <key>CFBundleDisplayName</key><string>Vario</string>
  <key>CFBundleIdentifier</key><string>io.github.kdev1966.vario</string>
  <key>CFBundleExecutable</key><string>vario-gui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleLocalizations</key><array><string>en</string><string>fr</string></array>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF

codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

archive="dist/Vario-macos-$arch.zip"
rm -f "$archive"
ditto -c -k --keepParent "$app" "$archive"
echo "Built $app and $archive (Vario $version, $arch)"
