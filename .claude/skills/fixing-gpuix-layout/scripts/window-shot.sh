#!/usr/bin/env bash
# Screenshot the running superterminal window, or a region inside it.
#
#   window-shot.sh                        -> /tmp/st-window.png, whole window
#   window-shot.sh out.png                -> whole window
#   window-shot.sh out.png DX DY W H      -> region, offsets relative to the
#                                            window's top-left, in POINTS
#
# WHY THIS EXISTS: a plain `screencapture` of the screen usually does not show
# the app at all — the window sits behind whatever else is open, or on another
# Space. So resolve the window's real CGWindowID and capture THAT.
#
# WHY `-l <windowid>` AND NOT `-R x,y,w,h`: `-R` captures a screen RECTANGLE, so
# any window stacked above the app bleeds into the image and you end up
# reviewing someone else's pixels. `-l` captures the window itself, correctly,
# even while it is fully occluded — which it usually is while you work.
#
# AppleScript/System Events is not an option here (no assistive access), and
# `sips --cropOffset` crops from the CENTRE rather than the top-left, so regions
# are cropped with PIL instead.
#
# Captures are Retina 2x, which is what makes a few points of misalignment
# visible. Read the PNG afterwards — do not fix layout blind.
set -euo pipefail

OUT="${1:-/tmp/st-window.png}"
SWIFT_SRC="$(mktemp -t st-winbounds).swift"
trap 'rm -f "$SWIFT_SRC"' EXIT

cat > "$SWIFT_SRC" <<'SWIFT'
import CoreGraphics
import Foundation

// From source the client runs as `bun`; a `bun build --compile` bundle runs
// under its own name (`superterminal`), so match either or this silently fails
// to find a packaged build. Layer 0 skips menu bars, shadows and other chrome
// that also belongs to the process. Require a plausible window size too: the
// process can own tiny offscreen helper windows that would otherwise win.
let opts = CGWindowListOption(arrayLiteral: .optionOnScreenOnly, .excludeDesktopElements)
guard let list = CGWindowListCopyWindowInfo(opts, kCGNullWindowID) as? [[String: Any]] else {
    FileHandle.standardError.write("window-shot: could not read the window list\n".data(using: .utf8)!)
    exit(1)
}
for w in list {
    let owner = ((w[kCGWindowOwnerName as String] as? String) ?? "").lowercased()
    let layer = (w[kCGWindowLayer as String] as? Int) ?? -1
    guard owner == "bun" || owner == "superterminal", layer == 0,
          let num = w[kCGWindowNumber as String] as? Int,
          let b = w[kCGWindowBounds as String] as? [String: Any] else { continue }
    let wd = (b["Width"] as? Double) ?? 0
    let ht = (b["Height"] as? Double) ?? 0
    guard wd >= 200, ht >= 200 else { continue }
    print("\(num) \(Int(wd)) \(Int(ht))")
    exit(0)
}
FileHandle.standardError.write("window-shot: no superterminal window on screen — is the app running?\n".data(using: .utf8)!)
exit(2)
SWIFT

read -r WIN_ID WW WH < <(xcrun swift "$SWIFT_SRC")

# -o drops the window shadow, so the image starts exactly at the window's edge
# and the offsets you pass line up with what the app actually laid out.
screencapture -x -o -l "$WIN_ID" "$OUT"

if [ "$#" -ge 5 ]; then
  python3 - "$OUT" "$2" "$3" "$4" "$5" "$WW" <<'PY'
import sys
from PIL import Image

out, dx, dy, w, h, win_w_pt = sys.argv[1], *map(int, sys.argv[2:7])
img = Image.open(out)
# The capture is in PIXELS and the offsets are in POINTS; derive the backing
# scale from the image rather than assuming 2x, so this stays correct on a
# non-Retina display or a scaled mode.
scale = img.width / win_w_pt if win_w_pt else 1.0
box = (round(dx * scale), round(dy * scale),
       round((dx + w) * scale), round((dy + h) * scale))
box = (max(0, box[0]), max(0, box[1]), min(img.width, box[2]), min(img.height, box[3]))
img.crop(box).save(out)
print(f"    cropped to {box} at scale {scale:g}")
PY
fi

echo "$OUT  (window ${WW}x${WH} pt, CGWindowID $WIN_ID)"
