#!/bin/sh
# Builds dist/Goat-<version>-x86_64.AppImage from dist/Goat-linux-x64.
# Needs appimagetool (https://github.com/AppImage/appimagetool) on PATH or in $APPIMAGETOOL.
set -e
cd "$(dirname "$0")/../.."
VERSION=$(node -p "require('./package.json').version")
SRC="dist/Goat-linux-x64"
APPDIR="dist/Goat.AppDir"
TOOL="${APPIMAGETOOL:-appimagetool}"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/lib/goat" "$APPDIR/usr/share/icons/hicolor/256x256/apps" "$APPDIR/usr/share/applications"
cp -a "$SRC/." "$APPDIR/usr/lib/goat/"
cp installer/linux/AppRun "$APPDIR/AppRun"
cp installer/linux/goat.desktop "$APPDIR/goat.desktop"
cp installer/linux/goat.desktop "$APPDIR/usr/share/applications/"
cp src/icon.png "$APPDIR/goat.png"
cp src/icon.png "$APPDIR/usr/share/icons/hicolor/256x256/apps/goat.png"
ln -sf goat.png "$APPDIR/.DirIcon"
chmod +x "$APPDIR/AppRun"
chmod -R a+rX "$APPDIR"
ARCH=x86_64 "$TOOL" --appimage-extract-and-run "$APPDIR" "dist/Goat-$VERSION-x86_64.AppImage" 2>/dev/null || ARCH=x86_64 "$TOOL" "$APPDIR" "dist/Goat-$VERSION-x86_64.AppImage"
echo "Built dist/Goat-$VERSION-x86_64.AppImage"
