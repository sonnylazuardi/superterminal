#!/usr/bin/env bash
# Start superterminal: the daemon (if not already up) and the GUI client.
#
#   scripts/run.sh            # build what is missing, then run
#   scripts/run.sh --no-build # run what is already built
#
# On WSL2 the window appears on your Windows desktop via WSLg.
set -euo pipefail

ST_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ST_ROOT"
# shellcheck source=./env.sh
source scripts/env.sh

BUILD=1
[ "${1:-}" = "--no-build" ] && BUILD=0

log() { printf '\033[36m==>\033[0m %s\n' "$*"; }

# WSLg exposes both Wayland and X11. On Wayland, GPUI uses client-side
# decorations and draws no window controls, so you get a borderless window you
# cannot close or resize. Under X11, WSLg hands the window to Windows, which
# draws a real title bar (close/minimise/maximise) and resize borders. So on
# WSL we default to X11. Set ST_FORCE_WAYLAND=1 to opt back in.
if [ -n "${WSL_DISTRO_NAME:-}" ] && [ "${ST_FORCE_WAYLAND:-0}" != "1" ] && [ -n "${DISPLAY:-}" ]; then
  if [ -n "${WAYLAND_DISPLAY:-}" ]; then
    log "using the X11 backend for native window decorations (ST_FORCE_WAYLAND=1 to override)"
  fi
  unset WAYLAND_DISPLAY
fi

if [ ! -d "$ST_SYSROOT" ] && ! pkg-config --exists fontconfig 2>/dev/null; then
  echo "error: no GPU/dev libraries found." >&2
  echo "  Install them (see docs/DEV.md §1) or create the sysroot (docs/DEV.md §4)." >&2
  exit 1
fi

if [ "$BUILD" = 1 ]; then
  log "building the daemon and CLI"
  cargo build -p st-server -p st-cli
  if [ ! -f "${NAPI_RS_NATIVE_LIBRARY_PATH:-/nonexistent}" ]; then
    log "building the native module (first build takes several minutes)"
    (cd crates/st-native && bun run build)
    source scripts/env.sh   # pick up the freshly built .node
  fi
fi

if [ -z "${NAPI_RS_NATIVE_LIBRARY_PATH:-}" ]; then
  echo "error: no native module found; run: (cd crates/st-native && bun run build)" >&2
  exit 1
fi

mkdir -p "$(dirname "$SUPERTERMINAL_SOCKET")"
if [ -S "$SUPERTERMINAL_SOCKET" ] && ./target/debug/st status >/dev/null 2>&1; then
  log "daemon already running at $SUPERTERMINAL_SOCKET"
else
  log "starting superterminald"
  ./target/debug/superterminald --foreground --socket "$SUPERTERMINAL_SOCKET" \
    >"${TMPDIR:-/tmp}/superterminald.log" 2>&1 &
  for _ in $(seq 1 50); do
    [ -S "$SUPERTERMINAL_SOCKET" ] && break
    sleep 0.1
  done
  ./target/debug/st status >/dev/null 2>&1 \
    || { echo "daemon failed to start; see ${TMPDIR:-/tmp}/superterminald.log" >&2; exit 1; }
fi

log "launching the client"
exec bun packages/app/src/app.tsx "$@"
