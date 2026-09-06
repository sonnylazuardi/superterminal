#!/usr/bin/env bash
# Build superterminal.app and a .dmg for macOS (arm64).
#
#   scripts/package-macos.sh              # build everything, then the dmg
#   scripts/package-macos.sh --no-build   # package what is already built
#
# This is the M6 packaging task brought forward far enough to hand someone a
# .dmg they can install. What it is NOT: signed with a Developer ID, or
# notarised. The bundle gets an ad-hoc signature (`codesign -s -`), which is
# enough for macOS to run it locally but NOT enough for Gatekeeper to accept a
# download — see the note this script prints at the end.
#
# Layout produced (05 §10: the daemon sits beside the client, because
# `ensureServer` resolves it from `dirname(process.execPath)`):
#
#   dist/superterminal.app/Contents/
#     Info.plist
#     MacOS/superterminal          the bun --compile binary (client)
#     MacOS/superterminald         the daemon
#     MacOS/st                     the CLI, handy for support
#     Resources/superterminal.icns  pre-Tahoe Dock/Finder icon
#     Resources/Assets.car          macOS 26 Liquid Glass (compiled from .icon)
set -euo pipefail

ST_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." && pwd)"
cd "$ST_ROOT"
# shellcheck source=./env.sh
source scripts/env.sh

BUILD=1
[ "${1:-}" = "--no-build" ] && BUILD=0

APP_NAME="superterminal"
BUNDLE_ID="dev.sonnylazuardi.superterminal"
DIST="$ST_ROOT/dist"
APP="$DIST/$APP_NAME.app"
MACOS_DIR="$APP/Contents/MacOS"
RES_DIR="$APP/Contents/Resources"

VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
GIT_SHA="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
BUILD_ID="$VERSION+$GIT_SHA"

log() { printf '\033[36m==>\033[0m %s\n' "$*"; }

[ "$(uname -s)" = "Darwin" ] || { echo "error: macOS only" >&2; exit 1; }

if [ "$BUILD" = 1 ]; then
  log "building the daemon and CLI (release)"
  CARGO_PROFILE_RELEASE_DEBUG=0 cargo build --release -p st-server -p st-cli

  log "building the native module (release, toolchain $ST_NATIVE_TOOLCHAIN)"
  (cd crates/st-native && CARGO_PROFILE_RELEASE_DEBUG=0 cargo "+$ST_NATIVE_TOOLCHAIN" build --release)
  mkdir -p packages/native
  cp crates/st-native/target/release/libst_native.dylib \
    "packages/native/superterminal-native.$ST_TRIPLE.node"
fi

NODE_ASSET="packages/native/superterminal-native.$ST_TRIPLE.node"
[ -f "$NODE_ASSET" ] || { echo "error: missing $NODE_ASSET (run without --no-build)" >&2; exit 1; }
for bin in superterminald st; do
  [ -f "target/release/$bin" ] || { echo "error: missing target/release/$bin" >&2; exit 1; }
done

log "compiling the client into a standalone binary"
rm -rf "$DIST"
mkdir -p "$MACOS_DIR" "$RES_DIR"
# --asset embeds the addon under /$bunfs/root/<relative path>; native/locate.ts
# detects the compiled case and copies it out to the user cache before dlopen,
# because loading a Node-API addon directly from /$bunfs/ is unverified (V6).
SUPERTERMINAL_BUILD_ID="$BUILD_ID" SUPERTERMINAL_VERSION="$VERSION" \
  bun build --compile packages/app/src/app.tsx \
    --asset "$NODE_ASSET" \
    --outfile "$MACOS_DIR/$APP_NAME"
chmod +x "$MACOS_DIR/$APP_NAME"

log "assembling the bundle"
cp target/release/superterminald "$MACOS_DIR/superterminald"
cp target/release/st "$MACOS_DIR/st"
# Also ship the addon on disk. locate.ts probes beside the executable, so this
# is the fallback if the embedded asset cannot be staged for any reason.
cp "$NODE_ASSET" "$MACOS_DIR/"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>superterminal</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleExecutable</key><string>$APP_NAME</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <!-- GPUI renders at native scale; without this the window is upscaled and blurry. -->
  <key>NSHighResolutionCapable</key><true/>
  <!-- A real windowed app, not an agent: it needs a Dock tile and menu bar. -->
  <key>LSUIElement</key><false/>
  <key>NSSupportsAutomaticTermination</key><false/>
  <key>NSSupportsSuddenTermination</key><false/>
</dict>
</plist>
PLIST

# Icons. Ship both formats: .icns for macOS < 26, Assets.car for Tahoe's
# Liquid Glass. actool must receive the .icon as a *standalone* input (nesting
# it in an .xcassets catalog silently produces nothing) and
# --output-partial-info-plist is required or compilation is skipped. --app-icon
# must match the package stem. We copy only Assets.car from the compile dir —
# actool also emits a small flattened .icns that would overwrite our
# high-res fallback.
# https://github.com/blackboardsh/electrobun/issues/121
# https://www.hendrik-erz.de/post/supporting-liquid-glass-icons-in-apps-without-xcode
# https://github.com/electron/packager/blob/main/src/icon-composer.ts
HAVE_ICON=0
if [ -f assets/superterminal.icns ]; then
  cp assets/superterminal.icns "$RES_DIR/"
  /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string superterminal" \
    "$APP/Contents/Info.plist" >/dev/null
  HAVE_ICON=1
fi
if [ -d assets/superterminal.icon ]; then
  if xcrun --find actool >/dev/null 2>&1; then
    log "compiling liquid glass icon"
    COMPILE_DIR="$(mktemp -d)"
    PLIST_TMP="$(mktemp)"
    if xcrun actool assets/superterminal.icon \
        --compile "$COMPILE_DIR" \
        --output-format human-readable-text \
        --notices --warnings --errors \
        --output-partial-info-plist "$PLIST_TMP" \
        --app-icon superterminal \
        --include-all-app-icons \
        --enable-on-demand-resources NO \
        --development-region en \
        --target-device mac \
        --minimum-deployment-target 26.0 \
        --platform macosx; then
      # actool can exit 0 while producing nothing: on Xcode whose SDK stops
      # below the 26.0 deployment target it prints a warning and skips the
      # compile. Copying blindly trips `set -e` on the missing file, so
      # check first and fall back to .icns like any other actool failure.
      if [ -f "$COMPILE_DIR/Assets.car" ]; then
        cp "$COMPILE_DIR/Assets.car" "$RES_DIR/"
        /usr/libexec/PlistBuddy -c "Add :CFBundleIconName string superterminal" \
          "$APP/Contents/Info.plist" >/dev/null
        HAVE_ICON=1
      else
        log "actool produced no Assets.car (SDK older than --minimum-deployment-target 26.0?) — shipping .icns only"
      fi
    else
      log "actool failed — shipping .icns only"
    fi
    rm -rf "$COMPILE_DIR" "$PLIST_TMP"
  else
    log "actool not found — shipping .icns only (no Liquid Glass)"
  fi
fi
if [ "$HAVE_ICON" = 0 ]; then
  log "no app icon assets — the app will use the generic icon"
fi

# Ad-hoc signature. Not a Developer ID, so this does not satisfy Gatekeeper for
# a downloaded app, but it keeps macOS from killing the binary outright and
# makes the bundle self-consistent after we edited its contents.
log "ad-hoc signing"
codesign --force --deep --sign - "$APP" 2>&1 | sed 's/^/    /' || true

log "building the dmg"
DMG="$DIST/$APP_NAME-$VERSION-arm64.dmg"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
# NOT piped through `sed`: with a pipeline, `set -o pipefail` still reports the
# LAST command's status, so a `hdiutil: create failed - No space left on device`
# was exiting 0 and leaving a truncated .dmg that looked like a successful build.
if ! hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG"; then
  rm -f "$DMG"   # never leave a partial image behind: it looks installable
  echo "error: hdiutil failed. A UDZO image needs roughly the bundle size again" >&2
  echo "       in free space (bundle is $(du -sh "$APP" | cut -f1)); free some and retry." >&2
  exit 1
fi

log "done"
echo
echo "  app: $APP"
echo "  dmg: $DMG  ($(du -h "$DMG" | cut -f1))"
echo
echo "  This build is ad-hoc signed and NOT notarised, so on first launch macOS"
echo "  will refuse it with \"cannot be opened because the developer cannot be"
echo "  verified\". To open it anyway, either:"
echo "    - right-click the app in /Applications and choose Open, then confirm; or"
echo "    - xattr -dr com.apple.quarantine /Applications/$APP_NAME.app"
