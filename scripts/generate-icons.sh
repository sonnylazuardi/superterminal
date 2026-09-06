#!/usr/bin/env bash
# Build assets/superterminal.icns, assets/superterminal.icon,
# assets/superterminal.ico (Windows) and assets/icon-preview.png (flattened
# Liquid Glass render) from the source PNG.
#
#   scripts/generate-icons.sh
#   scripts/generate-icons.sh path/to/source.png
#
# The .ico is cut square around the artwork's bounding box (the 1024 canvas
# leaves ~16% of transparent padding on every side, which reads as a tiny
# glyph in a 16 px tile) and holds 16..256 px PNG-compressed entries. It is
# the only step that runs on Linux too: iconutil and ictool are Mac-only.
#
# The .icns is the pre-Tahoe Dock/Finder fallback: the artwork composited
# onto the charcoal fill so transparent padding does not punch a hole in the
# squircle. The .icon is an Icon Composer package (icon.json + Assets/) that
# actool compiles to Assets.car for macOS 26 Liquid Glass. See
# scripts/package-macos.sh for the compile step.
set -euo pipefail

ST_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." && pwd)"
cd "$ST_ROOT"

SRC="${1:-assets/superterminal.png}"
[ -f "$SRC" ] || { echo "error: missing $SRC" >&2; exit 1; }

python3 - "$SRC" "$ST_ROOT" <<'PY'
import json, shutil, sys, tempfile
from pathlib import Path
from PIL import Image

src = Path(sys.argv[1]).resolve()
root = Path(sys.argv[2])
assets = root / "assets"
assets.mkdir(parents=True, exist_ok=True)

im = Image.open(src).convert("RGBA")
if im.size != (1024, 1024):
    im = im.resize((1024, 1024), Image.Resampling.LANCZOS)

master = assets / "superterminal.png"
im.save(master, "PNG")

# Charcoal matching the terminal face. Used as the .icns backdrop and as the
# Icon Composer fill so Tahoe's squircle and the legacy icon agree.
FILL = (24, 30, 28, 255)
legacy = Image.new("RGBA", (1024, 1024), FILL)
legacy.alpha_composite(im)

iconset = Path(tempfile.mkdtemp(prefix="st-iconset-")) / "superterminal.iconset"
iconset.mkdir()
for px, name in [
    (16, "icon_16x16.png"),
    (32, "icon_16x16@2x.png"),
    (32, "icon_32x32.png"),
    (64, "icon_32x32@2x.png"),
    (128, "icon_128x128.png"),
    (256, "icon_128x128@2x.png"),
    (256, "icon_256x256.png"),
    (512, "icon_256x256@2x.png"),
    (512, "icon_512x512.png"),
    (1024, "icon_512x512@2x.png"),
]:
    legacy.resize((px, px), Image.Resampling.LANCZOS).save(iconset / name, "PNG")
(Path(tempfile.gettempdir()) / "st-iconset-path").write_text(str(iconset))

icon_dir = assets / "superterminal.icon"
if icon_dir.exists():
    shutil.rmtree(icon_dir)
(icon_dir / "Assets").mkdir(parents=True)
shutil.copy2(master, icon_dir / "Assets" / "superterminal.png")

# Icon Composer 2.0 document. The 3D artwork already has baked lighting, so
# it is a non-glass layer sitting on a Liquid Glass enclosure (fill + group
# specular / translucency). Do not set both `fill` and `fill-specializations`.
# https://github.com/wendyliga/skills/blob/main/skills/icon-composer/SKILL.md
# https://github.com/electron/packager/blob/main/src/icon-composer.ts
fill = "extended-srgb:0.09412,0.11765,0.10980,1.00000"
fill_dark = "extended-srgb:0.07059,0.08627,0.08235,1.00000"
doc = {
    "fill-specializations": [
        {"value": {"automatic-gradient": fill}},
        {"appearance": "dark", "value": {"automatic-gradient": fill_dark}},
    ],
    "groups": [
        {
            "name": "Terminal",
            "blur-material": 0.4,
            "layers": [
                {
                    "name": "Artwork",
                    "image-name": "superterminal.png",
                    "glass": False,
                }
            ],
            "lighting": "combined",
            "shadow": {"kind": "neutral", "opacity": 0.45},
            "specular": True,
            "translucency": {"enabled": True, "value": 0.18},
        }
    ],
    "supported-platforms": {"squares": ["macOS"]},
}
(icon_dir / "icon.json").write_text(json.dumps(doc, indent=2) + "\n")

# Windows .ico: bun build --compile --windows-icon and the WiX shortcut /
# Apps & features entry (packaging/windows/Product.wxs).
l, t, r, b = im.getbbox()
side = max(r - l, b - t)
pad = int(side * 0.04)
side += 2 * pad
cx, cy = (l + r) // 2, (t + b) // 2
x0, y0 = cx - side // 2, cy - side // 2
square = Image.new("RGBA", (side, side), (0, 0, 0, 0))
square.paste(im, (-x0, -y0))
ico = assets / "superterminal.ico"
square.save(ico, "ICO", sizes=[(px, px) for px in (256, 128, 64, 48, 40, 32, 24, 20, 16)])
print(f"wrote {ico}")
print(f"wrote {master}")
print(f"wrote {icon_dir}")
print(f"iconset {iconset}")
PY

ICONSET="$(python3 -c 'from pathlib import Path; import tempfile; print((Path(tempfile.gettempdir()) / "st-iconset-path").read_text())')"
if ! command -v iconutil >/dev/null 2>&1; then
  rm -rf "$(dirname "$ICONSET")"
  echo "warning: iconutil not found (not macOS) — skipped .icns and icon-preview.png" >&2
  exit 0
fi
iconutil -c icns "$ICONSET" -o assets/superterminal.icns
echo "wrote assets/superterminal.icns ($(du -h assets/superterminal.icns | cut -f1))"
rm -rf "$(dirname "$ICONSET")"

# Flattened Liquid Glass preview (what Tahoe actually composites). ictool ships
# inside Icon Composer; without it we skip rather than inventing a fake render.
ICTOOL="$(dirname "$(xcode-select -p)")/Applications/Icon Composer.app/Contents/Executables/ictool"
if [ -x "$ICTOOL" ]; then
  "$ICTOOL" assets/superterminal.icon \
    --export-image --output-file assets/icon-preview.png \
    --platform macOS --rendition Default --width 1024 --height 1024 --scale 2
  echo "wrote assets/icon-preview.png ($(du -h assets/icon-preview.png | cut -f1))"
else
  echo "warning: ictool not found — skipped assets/icon-preview.png" >&2
fi
