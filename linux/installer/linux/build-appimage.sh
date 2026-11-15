#!/bin/sh
# Builds dist/Goatr-<version>-x86_64.AppImage from dist/Goatr-linux-x64.
# Needs appimagetool (https://github.com/AppImage/appimagetool) on PATH or in $APPIMAGETOOL.
set -e
cd "$(dirname "$0")/../.."
VERSION=$(node -p "require('./package.json').version")
SRC="dist/Goatr-linux-x64"
APPDIR="dist/Goatr.AppDir"
TOOL="${APPIMAGETOOL:-appimagetool}"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/lib/goatr" "$APPDIR/usr/share/icons/hicolor/256x256/apps" "$APPDIR/usr/share/applications"
cp -a "$SRC/." "$APPDIR/usr/lib/goatr/"
cp installer/linux/AppRun "$APPDIR/AppRun"
cp installer/linux/goatr.desktop "$APPDIR/goatr.desktop"
cp installer/linux/goatr.desktop "$APPDIR/usr/share/applications/"
cp src/icon.png "$APPDIR/goatr.png"
cp src/icon.png "$APPDIR/usr/share/icons/hicolor/256x256/apps/goatr.png"
ln -sf goatr.png "$APPDIR/.DirIcon"
chmod +x "$APPDIR/AppRun"
chmod -R a+rX "$APPDIR"
ARCH=x86_64 "$TOOL" --appimage-extract-and-run "$APPDIR" "dist/Goatr-$VERSION-x86_64.AppImage" 2>/dev/null || ARCH=x86_64 "$TOOL" "$APPDIR" "dist/Goatr-$VERSION-x86_64.AppImage"
echo "Built dist/Goatr-$VERSION-x86_64.AppImage"
