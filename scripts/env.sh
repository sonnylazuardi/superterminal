#!/usr/bin/env bash
# Shared environment for building and running superterminal on this machine.
#
# WHY THIS EXISTS: on Linux the GPUI stack links against system -dev packages
# (fontconfig, xkbcommon, vulkan, wayland, xcb). If you have root, install them
# properly with the apt one-liner in docs/DEV.md §1 and that part becomes a
# no-op. Without root we unpack those .debs into a sysroot and point the build
# and the loader at it. On macOS none of that applies: Metal, CoreText and
# AppKit come with the SDK.
#
# Usage:  source scripts/env.sh
set -u

# `$0` rather than a bare `${BASH_SOURCE[0]}`: macOS defaults to zsh, which has
# no BASH_SOURCE, and `set -u` then aborts the source with
# "BASH_SOURCE[0]: parameter not set". zsh sets `$0` to the sourced file.
ST_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." && pwd)"
ST_OS="$(uname -s)"

# napi-rs platform triple. Must match `napiTriple()` in
# packages/app/src/native/locate.ts, which is what the app probes for.
if [ -z "${ST_TRIPLE:-}" ]; then
  case "$ST_OS" in
    Darwin)
      case "$(uname -m)" in
        arm64) ST_TRIPLE="darwin-arm64" ;;
        *)     ST_TRIPLE="darwin-x64" ;;
      esac
      ;;
    *) ST_TRIPLE="linux-x64-gnu" ;;
  esac
fi
export ST_TRIPLE

# The toolchain vendor/gpuix pins. rustup resolves rust-toolchain.toml from the
# INVOCATION directory, and the nearest file to crates/st-native is the repo
# root's (`stable`) — vendor/gpuix is not an ancestor of it. So the pin has to
# be passed explicitly; docs/DEV.md §1 used to claim otherwise.
ST_NATIVE_TOOLCHAIN="${ST_NATIVE_TOOLCHAIN:-1.97.1}"
export ST_NATIVE_TOOLCHAIN

# rustup installed with --no-modify-path leaves no shell profile entry, so a
# fresh shell has no `cargo`. Add it here rather than making every caller
# remember `. "$HOME/.cargo/env"`.
if [ -d "$HOME/.cargo/bin" ] && ! command -v cargo >/dev/null 2>&1; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

ST_SYSROOT="${ST_SYSROOT:-$HOME/.local/share/superterminal/sysroot}"
export ST_SYSROOT

if [ "$ST_OS" != "Darwin" ] && [ -d "$ST_SYSROOT" ]; then
  export PKG_CONFIG_PATH="$ST_SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$ST_SYSROOT/usr/share/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  export PKG_CONFIG_SYSROOT_DIR="$ST_SYSROOT"
  export CPATH="$ST_SYSROOT/usr/include:$ST_SYSROOT/usr/include/x86_64-linux-gnu${CPATH:+:$CPATH}"
  export LIBRARY_PATH="$ST_SYSROOT/usr/lib/x86_64-linux-gnu${LIBRARY_PATH:+:$LIBRARY_PATH}"
  export LD_LIBRARY_PATH="$ST_SYSROOT/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  export PATH="$ST_SYSROOT/usr/bin:$PATH"
fi

# Where `just build-native` / scripts/run.sh put the addon, and the first path
# locate.ts probes. The older dist/ location is still honoured.
ST_NODE="$ST_ROOT/packages/native/superterminal-native.$ST_TRIPLE.node"
[ -f "$ST_NODE" ] || ST_NODE="$ST_ROOT/crates/st-native/dist/superterminal-native.$ST_TRIPLE.node"
export ST_NODE
if [ -f "$ST_NODE" ]; then
  # gpuix's napi loader checks this before anything else, which is how our
  # addon substitutes for the stock @gpuix/native prebuilt. bunfig.toml's
  # top-level `preload` does the same thing for a plain `bun` invocation.
  export NAPI_RS_NATIVE_LIBRARY_PATH="$ST_NODE"
fi

# The socket, derived EXACTLY as crates/st-config/src/paths.rs does it:
#   $SUPERTERMINAL_SOCKET -> $XDG_RUNTIME_DIR/superterminal
#                         -> $TMPDIR/superterminal-<uid> -> /tmp/superterminal-<uid>
# Do not "simplify" this to $XDG_RUNTIME_DIR-or-/tmp: on macOS XDG_RUNTIME_DIR
# is unset and the daemon listens under $TMPDIR, so the two would disagree and
# the client would never find a running server.
if [ -z "${SUPERTERMINAL_SOCKET:-}" ]; then
  if [ -n "${XDG_RUNTIME_DIR:-}" ]; then
    ST_RUNTIME_DIR="$XDG_RUNTIME_DIR/superterminal"
  else
    ST_RUNTIME_DIR="${TMPDIR:-/tmp}/superterminal-$(id -u)"
  fi
  SUPERTERMINAL_SOCKET="${ST_RUNTIME_DIR%/}/server.sock"
fi
export SUPERTERMINAL_SOCKET
