Goat for Linux

Install (no sudo needed):
  tar xf Goat-*-linux-x64.tar.xz
  cd goat && ./install.sh

That puts Goat in your app menu and on your desktop, and adds a "goat" command.
Your data (settings, history, profiles) is kept in ~/.config/Goat.
Uninstall: ~/.local/opt/goat/install.sh --uninstall  (keeps your data)

Or skip installing and just run ./goat.sh from this folder.

Chrome extensions are not supported on Linux.

Sandbox: Chromium's sandbox needs unprivileged user namespaces. Some systems
(Ubuntu 24.04+) block them, and then Goat starts without the sandbox. To keep it on:
  sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
