#!/bin/sh
# Installs Goatr for your user (no sudo): app in ~/.local/opt/goatr, a "goatr" command,
# a menu entry and a desktop shortcut. Your data stays in ~/.config/Goatr.
#   ./install.sh              install or update
#   ./install.sh --uninstall  remove Goatr (keeps your data)
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
DEST="$HOME/.local/opt/goatr"
BIN="$HOME/.local/bin"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/256x256/apps"
DESK="$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")"

if [ "$1" = "--uninstall" ]; then
  rm -rf "$DEST" "$BIN/goatr" "$APPS/goatr.desktop" "$ICONS/goatr.png" "$DESK/goatr.desktop"
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS" 2>/dev/null || true
  echo "Goatr was removed. Your data is still in ~/.config/Goatr (delete that folder to remove it too)."
  exit 0
fi

echo "Installing Goatr to $DEST…"
mkdir -p "$DEST" "$BIN" "$APPS" "$ICONS"
if [ "$HERE" != "$DEST" ]; then
  rm -rf "$DEST.new" && cp -a "$HERE" "$DEST.new" && rm -rf "$DEST" && mv "$DEST.new" "$DEST"
fi
chmod +x "$DEST/goatr" "$DEST/goatr.sh" "$DEST/install.sh"
ln -sf "$DEST/goatr.sh" "$BIN/goatr"
cp "$DEST/goatr.png" "$ICONS/goatr.png"
sed "s#^Exec=goatr#Exec=$DEST/goatr.sh#; s#^Icon=goatr#Icon=$ICONS/goatr.png#" "$DEST/goatr.desktop" > "$APPS/goatr.desktop"
chmod +x "$APPS/goatr.desktop"
if [ -d "$DESK" ]; then
  cp "$APPS/goatr.desktop" "$DESK/goatr.desktop"
  chmod +x "$DESK/goatr.desktop"
  # GNOME: mark the desktop shortcut as trusted so it opens on double-click.
  command -v gio >/dev/null 2>&1 && gio set "$DESK/goatr.desktop" metadata::trusted true 2>/dev/null || true
fi
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS" 2>/dev/null || true
echo "Done. Open Goatr from your app menu, the desktop shortcut, or run: goatr"
