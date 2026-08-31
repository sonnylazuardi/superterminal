#!/usr/bin/env bash
# Shared environment for building and running superterminal on this machine.
#
# WHY THIS EXISTS: the GPUI stack links against system -dev packages
# (fontconfig, xkbcommon, vulkan, wayland, xcb). If you have root, install them
# properly with the apt one-liner in docs/DEV.md §1 and this file becomes a
# no-op. Without root we unpack those .debs into a sysroot and point the build
# and the loader at it.
#
# Usage:  source scripts/env.sh
set -u

ST_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ST_SYSROOT="${ST_SYSROOT:-$HOME/.local/share/superterminal/sysroot}"
ST_TRIPLE="${ST_TRIPLE:-linux-x64-gnu}"

if [ -d "$ST_SYSROOT" ]; then
  export PKG_CONFIG_PATH="$ST_SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$ST_SYSROOT/usr/share/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  export PKG_CONFIG_SYSROOT_DIR="$ST_SYSROOT"
  export CPATH="$ST_SYSROOT/usr/include:$ST_SYSROOT/usr/include/x86_64-linux-gnu${CPATH:+:$CPATH}"
  export LIBRARY_PATH="$ST_SYSROOT/usr/lib/x86_64-linux-gnu${LIBRARY_PATH:+:$LIBRARY_PATH}"
  export LD_LIBRARY_PATH="$ST_SYSROOT/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  export PATH="$ST_SYSROOT/usr/bin:$PATH"
fi

# The npm prebuilt @gpuix/native-linux-x64-gnu is built against GLIBC 2.39 and
# will not load on older systems (this box is 2.35). Our own .node is a
# drop-in superset built locally, and gpuix's loader checks this variable
# before anything else.
ST_NODE="$ST_ROOT/crates/st-native/dist/superterminal-native.$ST_TRIPLE.node"
if [ -f "$ST_NODE" ]; then
  export NAPI_RS_NATIVE_LIBRARY_PATH="$ST_NODE"
fi

export SUPERTERMINAL_SOCKET="${SUPERTERMINAL_SOCKET:-${XDG_RUNTIME_DIR:-/tmp}/superterminal/server.sock}"
