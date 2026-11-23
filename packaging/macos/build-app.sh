#!/bin/sh
# Wrap a macOS release build in a .app. Does not cross-compile and does not sign.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
cd "$root"
cargo build -p neo-cli -p neo-desktop --release
app=dist/NEO.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/assets"
cp target/release/neo-desktop target/release/neo-cli "$app/Contents/MacOS/"
cp -R assets/fonts assets/presets assets/styles "$app/Contents/Resources/assets/"
cp NOTICE LICENSE LICENSE-MIT LICENSE-APACHE "$app/Contents/Resources/"
cp packaging/macos/Info.plist "$app/Contents/Info.plist"
printf '%s\n' '{"version":"0.1.0","url":"https://example.invalid/neo-0.1.0-macos.tar.gz"}' > "$app/Contents/Resources/update.json"
echo "wrote $app"
echo "cold start: $app/Contents/MacOS/neo-cli open --synth 1000000"
