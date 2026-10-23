#!/usr/bin/env bash
# Starts the Linux debug build of Muninn under xvfb inside the test container.
#   run-app.sh BINARY [BUNDLED_BIN_DIR]
# Env: HOME is reset to a fresh /tmp/home unless KEEP_HOME=1.
set -u
BIN="${1:-/cache/target/debug/muninn}"
TOOLS="${2:-/tmp/tools}"
pkill -x muninn 2>/dev/null; pkill -x Xvfb 2>/dev/null; sleep 0.5
[ "${KEEP_HOME:-0}" = 1 ] || { rm -rf /tmp/home /tmp/e2e; }
mkdir -p /tmp/home /tmp/e2e /tmp/shots
export HOME=/tmp/home DISPLAY=:99 MUNINN_E2E_DIR=/tmp/e2e MUNINN_BUNDLED_BIN="$TOOLS" MUNINN_PROXY_LOG=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_COMPOSITING_MODE=1 LIBGL_ALWAYS_SOFTWARE=1 GST_DEBUG=${GST_DEBUG:-1}
(Xvfb :99 -screen 0 1440x900x24 >/tmp/xvfb.log 2>&1 &)
sleep 1
(dbus-run-session -- "$BIN" "${@:3}" >/tmp/app.log 2>&1 &)
for i in $(seq 1 60); do [ -f /tmp/home/.local/share/io.github.iarnvidr.muninn/service.json ] && break; sleep 0.5; done
echo started
