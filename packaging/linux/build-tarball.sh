#!/bin/sh
# Package a Linux build that was produced on this machine. Does not cross-compile.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
cd "$root"
cargo build -p neo-cli -p neo-desktop --release
stage=$(mktemp -d)
mkdir -p "$stage/usr/bin" "$stage/usr/share/applications" "$stage/usr/share/neo/assets"
cp target/release/neo-cli target/release/neo-desktop "$stage/usr/bin/"
cp packaging/linux/neo.desktop "$stage/usr/share/applications/"
cp -R assets/fonts assets/presets assets/styles "$stage/usr/share/neo/assets/"
cp NOTICE LICENSE LICENSE-MIT LICENSE-APACHE "$stage/usr/share/neo/"
printf '%s\n' '{"version":"0.1.0","url":"https://example.invalid/neo-0.1.0-linux-x64.tar.gz"}' > "$stage/update.json"
mkdir -p dist
tar -C "$stage" -czf dist/neo-linux-x64.tar.gz .
rm -rf "$stage"
echo "wrote dist/neo-linux-x64.tar.gz"
