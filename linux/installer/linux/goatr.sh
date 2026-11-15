#!/bin/sh
# Goatr launcher: turns Chromium's sandbox off only when the system blocks it.
HERE="$(dirname "$(readlink -f "$0")")"
EXTRA=""
if [ -r /proc/sys/kernel/apparmor_restrict_unprivileged_userns ] && \
   [ "$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns)" = "1" ] && \
   [ ! -u "$HERE/chrome-sandbox" ]; then
  EXTRA="--no-sandbox"
elif command -v unshare >/dev/null 2>&1 && ! unshare -Ur true >/dev/null 2>&1 && [ ! -u "$HERE/chrome-sandbox" ]; then
  EXTRA="--no-sandbox"
fi
exec "$HERE/goatr" $EXTRA "$@"
