#!/bin/sh
# Installs Goat for your user (no sudo): app in ~/.local/opt/goat, a "goat" command,
# a menu entry and a desktop shortcut. Your data stays in ~/.config/Goat.
#   ./install.sh              install or update
#   ./install.sh --uninstall  remove Goat (keeps your data)
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
DEST="$HOME/.local/opt/goat"
BIN="$HOME/.local/bin"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/256x256/apps"
DESK="$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")"

if [ "$1" = "--uninstall" ]; then
  rm -rf "$DEST" "$BIN/goat" "$APPS/goat.desktop" "$ICONS/goat.png" "$DESK/goat.desktop"
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS" 2>/dev/null || true
  echo "Goat was removed. Your data is still in ~/.config/Goat (delete that folder to remove it too)."
  exit 0
fi

# Remove the old install from when the browser was called Goatr (your data moves on first start).
rm -rf "$HOME/.local/opt/goatr" "$BIN/goatr" "$APPS/goatr.desktop" "$ICONS/goatr.png" "$DESK/goatr.desktop"
echo "Installing Goat to $DEST…"
mkdir -p "$DEST" "$BIN" "$APPS" "$ICONS"
if [ "$HERE" != "$DEST" ]; then
  rm -rf "$DEST.new" && cp -a "$HERE" "$DEST.new" && rm -rf "$DEST" && mv "$DEST.new" "$DEST"
fi
chmod +x "$DEST/goat" "$DEST/goat.sh" "$DEST/install.sh"
ln -sf "$DEST/goat.sh" "$BIN/goat"
cp "$DEST/goat.png" "$ICONS/goat.png"
sed "s#^Exec=goat#Exec=$DEST/goat.sh#; s#^Icon=goat#Icon=$ICONS/goat.png#" "$DEST/goat.desktop" > "$APPS/goat.desktop"
chmod +x "$APPS/goat.desktop"
if [ -d "$DESK" ]; then
  cp "$APPS/goat.desktop" "$DESK/goat.desktop"
  chmod +x "$DESK/goat.desktop"
  # GNOME: mark the desktop shortcut as trusted so it opens on double-click.
  command -v gio >/dev/null 2>&1 && gio set "$DESK/goat.desktop" metadata::trusted true 2>/dev/null || true
fi
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS" 2>/dev/null || true
echo "Done. Open Goat from your app menu, the desktop shortcut, or run: goat"
