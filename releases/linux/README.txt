Goatr for Linux

Needs WebKitGTK 4.1 and GTK 3 (glibc 2.39 or newer, e.g. Ubuntu 24.04+):
  Debian/Ubuntu:  sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0
  Fedora:         sudo dnf install webkit2gtk4.1 gtk3
  Arch:           sudo pacman -S webkit2gtk-4.1 gtk3

Run:  chmod +x goatr && ./goatr
Data is kept in ~/.local/share/goatr (settings, history, profiles).

The Linux version uses WebKitGTK instead of WebView2, so Windows-only features
(Chrome extensions, Tor circuit view, audio for streaming, Goatr's right-click menu)
aren't available.
