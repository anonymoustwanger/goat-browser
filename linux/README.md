# Goatr for Linux

The Linux edition of Goatr, built on Electron (Chromium). Chrome extensions are not supported.

## Build

```sh
npm install
npm start            # run from source
npm run tar          # dist/Goatr-<version>-linux-x64.tar.xz (with install.sh)
npm run appimage     # dist/Goatr-<version>-x86_64.AppImage (needs appimagetool)
```

## Install

```sh
tar xf Goatr-*-linux-x64.tar.xz
cd goatr && ./install.sh
```

Installs for your user (no sudo) into `~/.local/opt/goatr`, adds a menu entry, a desktop shortcut and a `goatr` command. Data lives in `~/.config/Goatr`. Uninstall with `~/.local/opt/goatr/install.sh --uninstall` (keeps your data).

## Features

Ad blocker (Ghostery engine, three levels, custom rules, per-site allowlist), built-in Tor and Tor windows, private windows and tabs, profiles, encrypted DNS, fingerprint protection, HTTPS-only mode, cookie manager, proxy settings, tab folders, themes, bookmarks bar, history, translation, link cleaning, memory saver and automatic update checks.

## Sandbox

Chromium's sandbox needs unprivileged user namespaces. On systems that block them (Ubuntu 24.04+), Goatr starts without it. To keep it on: `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`.
