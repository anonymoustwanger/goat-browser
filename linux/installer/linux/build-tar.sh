#!/bin/sh
# Builds dist/Goatr-<version>-linux-x64.tar.xz from dist/Goatr-linux-x64 (with install.sh).
set -e
cd "$(dirname "$0")/../.."
VERSION=$(node -p "require('./package.json').version")
SRC="dist/Goatr-linux-x64"
cp installer/linux/install.sh installer/linux/goatr.sh installer/linux/goatr.desktop "$SRC/"
cp src/icon.png "$SRC/goatr.png"
cp installer/linux/README.txt "$SRC/README.txt"
chmod +x "$SRC/install.sh" "$SRC/goatr.sh" "$SRC/goatr"
rm -rf "dist/goatr" && cp -a "$SRC" "dist/goatr"
tar -C dist -cf - goatr | xz -9e -T0 > "dist/Goatr-$VERSION-linux-x64.tar.xz"
rm -rf "dist/goatr"
echo "Built dist/Goatr-$VERSION-linux-x64.tar.xz"
