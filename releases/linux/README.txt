Goatr for Linux

Install (no sudo needed):
  tar xf Goatr-*-linux-x64.tar.xz
  cd goatr && ./install.sh

That puts Goatr in your app menu and on your desktop, and adds a "goatr" command.
Your data (settings, history, profiles) is kept in ~/.config/Goatr.
Uninstall: ~/.local/opt/goatr/install.sh --uninstall  (keeps your data)

Or skip installing and just run ./goatr.sh from this folder.

Chrome extensions are not supported on Linux.

Sandbox: Chromium's sandbox needs unprivileged user namespaces. Some systems
(Ubuntu 24.04+) block them, and then Goatr starts without the sandbox. To keep it on:
  sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
