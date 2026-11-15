#!/bin/sh
# Builds dist/Goat-<version>-linux-x64.tar.xz from dist/Goat-linux-x64 (with install.sh).
set -e
cd "$(dirname "$0")/../.."
VERSION=$(node -p "require('./package.json').version")
SRC="dist/Goat-linux-x64"
cp installer/linux/install.sh installer/linux/goat.sh installer/linux/goat.desktop "$SRC/"
cp src/icon.png "$SRC/goat.png"
cp installer/linux/README.txt "$SRC/README.txt"
chmod +x "$SRC/install.sh" "$SRC/goat.sh" "$SRC/goat"
rm -rf "dist/goat" && cp -a "$SRC" "dist/goat"
tar -C dist -cf - goat | xz -9e -T0 > "dist/Goat-$VERSION-linux-x64.tar.xz"
rm -rf "dist/goat"
echo "Built dist/Goat-$VERSION-linux-x64.tar.xz"
