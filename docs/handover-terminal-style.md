# Handover: terminal rendering style (cells, chrome glass, palette)

Implementation notes for the "make it look like iTerm" pass, as landed on
`main`. It records what the pass found on an M-series Mac, what it changed,
what it deliberately did **not** change, and what is still open. Read it
before touching `crates/st-native/src/paint.rs`, `sprites.rs`,
`theme/tokens.ts`, or any `[font]`/`[theme]` default.

Companion docs: [`handover-ui-chrome.md`](./handover-ui-chrome.md) (the
sidebar/palette chrome this builds on),
[`handover-pointer-input.md`](./handover-pointer-input.md) (the mouse
coordinate fix, file drop and tab reordering that followed this pass — §1.4
and §1.5 below are summaries; the detail is there),
[ADR 0010](./adr/0010-box-drawing-is-drawn-not-shaped.md) (the decision behind
§1.1), and the
[`fixing-gpuix-layout`](../.claude/skills/fixing-gpuix-layout/SKILL.md) skill
(how to *see* the window on macOS, and which style props gpuix drops).

---

## 0. Goal

| | |
|---|---|
| Goal | Terminal cells and chrome render correctly against any window backdrop and any font — specifically, the block-art logos TUIs draw (`opencode`, Claude Code) tile with no seams, and the sidebar stays legible over a light window — and the pointer interactions iTerm users expect (click a TUI tab, drop a file) work |
| Non-goal | Pixel parity with iTerm2's *font*. See §2: it is not reachable by configuration, and one attempt made things worse |

**Done when:**

- A row of `▀▄█` in Claude Code's or `opencode`'s logo paints as one solid
  shape: no horizontal seam between rows, no fragment out of place.
- `╭─╮ │ ╰─╯` borders join without steps; the corners are round.
- The sidebar reads at the same contrast whether the window behind it is a
  dark editor or a white browser page.
- A `[theme]` block copied from `docs/config-example.toml` changes the
  **cells**, not just the daemon's OSC 10/11 answers.
- Clicking a session tab in OpenCode switches to it (mouse reports land on
  the cell under the pointer, not one down and one right).
- Dropping a PNG from Finder onto the grid types its escaped path plus a
  space; in OpenCode that becomes an `[Image 1]` attachment.
- `cd crates/st-native && cargo +1.97.1 test --lib` → 171 pass: 21 in
  `sprites::tests`, 7 in `dnd::tests`, and
  `element::tests::a_click_on_the_top_left_cell_reaches_the_wire_as_1_1`.

---

## 1. What changed, and the evidence for each

### 1.1 Box Drawing + Block Elements are drawn, not shaped

**Symptom.** Logos built from `▀▄█▌▐` and quadrants rendered *striped*: a
visible seam between every row of blocks, and with some fonts the shape
fragmented entirely. Borders made of `─│╭` had steps where cells met.

**Two root causes, both structural to shaping through the font:**

1. A font glyph fills the font's **em box**, not our **cell**. The cell is
   `max(size × line_height, ascent + descent)`; with the default `1.2` on
   Menlo 13 that is 15.6 px against a 15.13 px glyph box. The 0.47 px
   difference is the seam, and on Retina it is a whole device pixel.
2. The family may simply **not have the glyph**. Measured with CoreText:

   ```
   Monaco 12:  5/16 present   MISSING: ▀ ▄ ▌ ▐ ░ ▓ ▖ ▙ ▟ ╭ ━
   Menlo  13: 16/16 present
   ```

   Every missing glyph falls back to a different face whose metrics do not
   match the cell. That is the fragmentation.

**Fix.** Draw U+2500–U+259F ourselves from the cell box, as iTerm2
(`iTermBoxDrawingBezierCurveFactory`), Kitty, Ghostty (sprite font),
Alacritty (`builtin_box_drawing`), VTE and xterm.js all do. Alacritty's
commit message is the one-line justification: the font "tends to overlap or
not align, so providing built-in font is the lesser evil."

| File | Role |
|---|---|
| `crates/st-native/src/sprites.rs` | **Pure geometry, no gpui.** `is_sprite(ch)`, `sprite_for(ch, w, h) -> Option<Sprite>` (`rects_for` for the rect-only view) in cell-local px. Block Elements are exact eighths/halves/quadrants of the cell; the three shades are a full cell at 25/50/75 % alpha; Box Drawing is four arms (light / heavy) out from the centre, the 28 double-line forms hand-listed so outer joins outer and inner joins inner, plus the 12 dashed forms. `╭╮╯╰` come back as an `Arc`. Fully unit-tested headless. |
| `crates/st-native/src/runs.rs` | `RunSpan.sprite: bool`. A cell that is exactly one sprite codepoint (not wide, not a grapheme cluster) gets a sprite run; sprite runs merge only with sprite runs. They never enter the shaped-line cache. |
| `crates/st-native/src/paint.rs` | `paint_sprite_run` turns rects into `paint_quad(fill(..))`. `paint_arc` draws `╭╮╯╰` as a real quarter circle: one `quad()` with no fill, a border on the two joined edges, and a radius on their corner. A block cursor over a sprite repaints it from the same geometry in `cursorText`. |
| `crates/st-native/src/stats.rs` | `spriteQuads` in `stReadProp(id, 'stats')`: how many sprite quads the last frame painted. The headless signal that a logo took the sprite path. |

**The load-bearing detail — edges are `round()`ed, never `ceil()`ed.** Two
neighbouring cells derive their shared edge from the same grid line, so
rounding both lands them on the same integer: no seam, no overlap. The
background quads use `ceil()` and get away with it because overlap there is
invisible; a `ceil()`ed 1 px stroke becomes 2 px wherever it straddles a
pixel boundary and the weight wobbles along a border. A rect whose rounded
edges coincide is widened to 1 px rather than dropped.
(`paint::snap_rect`, tested by `sprite_edges_round_to_the_pixel_grid_and_never_vanish`.)

**Stroke weight scales with the cell.** Light is `round(w / 8)` px, at least 1;
heavy is `2 × light + 1`; the two strokes of a double line sit
`max(light, round(w / 6))` px either side of the centre. An 8 px cell gets
1 / 3 px; a 20 px cell 3 / 7 px.

**Left to the font, on purpose:** the diagonals `╱╲╳` (U+2571–U+2573). They
are not rectangles. `is_sprite` excludes them.

### 1.2 Chrome glass has its own ground

**Symptom.** The sidebar was legible over a dark window behind it and blank
over a light one.

**Cause.** Every chrome surface was filled with `bg.glass` = white at 5 %, and
the whole palette above it is white too (`fg.primary #F2F2F2`, `fg.muted`
50 % white, selected row 15 % white). That only reads if something dark is
behind it, and `windowBackground: 'blurred'` puts the user's desktop there.
GPUI offers exactly three materials — transparent, blurred, opaque — and all
three transmit the backdrop, so no material fixes this.

**Fix.** Token `bg.chrome`, a **dark scrim at 85 %** (`#1E1E24D9`), on
every surface that sits directly on the backdrop: sidebar column, content
column, title bar, tab strip, banner, divider band, horizontal frame. Fills
layered *inside* an already-grounded surface keep `bg.glass` (palette input,
session chip, pane behind the grid). Measured on the rendered window, worst
case: ground composites to RGB(54,54,59), `fg.primary` at **10.7:1**.
`opaqueTokens.bg.chrome` is the same colour as its `glass`, since nothing
shows through an opaque window.

**Do not lower the alpha casually.** `fg.muted` drops under 3.4:1 over a
white backdrop by ~78 %. The token comment carries the numbers.

### 1.3 The documented theme keys now reach the cells

**Bug.** `docs/config-example.toml` documents the palette as `black`, `red`
… `bright_white`, `selection_background`, `selection_foreground`, and
`st-config`'s `ThemeConfig` deserializes exactly those. The client's
`buildTerminalTheme` only ever read `ansi0`…`ansi15` and
`selection_bg`/`selectionBg`. `[theme]` is a free-form record, so a palette
copied from the example parsed clean, reached the daemon, answered OSC 10/11
correctly — and was dropped before the cells, with no warning on either side.
Only `foreground`/`background`/`cursor`/`cursor_text` happened to be spelled
the same, which made it look half-applied rather than ignored.

**Fix.** Both spellings accepted; `ansiN` wins if both appear
(`theme/tokens.ts`, `ansiKeyName`). Regression test in
`packages/app/src/platform/window-options.test.ts` feeds the exact block from
the example.

### 1.4 Mouse reports were off by one cell

**Symptom.** Clicking a TUI's tab (OpenCode's session tabs) did nothing;
the same click in iTerm switched tabs.

**Cause.** `report_cell` returned 1-based coordinates and `encode_mouse`
added the 1-based offset again — `MouseEvent::cell` is documented 0-based.
Every press, release, drag and wheel report went out at `(col+2, row+2)`, so
a click on the tab row landed on the blank line beneath it. A unit test
(`report_cells_are_one_based`) had pinned the wrong contract.

**Fix.** `report_cell` is 0-based. The test now asserts that, and a new one
runs a click through the same two functions the press handler calls and
checks the wire bytes: top-left is `ESC[<0;1;1M`. That end-to-end assertion
is the one that would have caught it. Detail in
[`handover-pointer-input.md`](./handover-pointer-input.md) §1.

### 1.5 Dropping files types their paths

Finder → terminal, as iTerm2 and Terminal.app do it: each dropped file's path
is escaped and typed into the shell followed by a space. This is what lets a
TUI such as OpenCode turn a dropped PNG into an `[Image]` attachment — it is
reading a pasted path, not receiving a file.

| File | Role |
|---|---|
| `crates/st-native/src/dnd.rs` | Pure: `shell_word(path)` / `shell_words(paths)`. POSIX: ASCII outside a conservative safe set is backslash-escaped; non-ASCII is left alone (the shell takes UTF-8 as-is). Windows: backslashes are path separators there and cmd/PowerShell do not read `\ `, so a path with a space or metacharacter is double-quoted instead. Tested headless for both dialects. |
| `crates/st-native/src/element.rs` | `on_drop::<gpui::ExternalPaths>` in `install_listeners`. Focuses the grid, then goes through `GridState::paste`, so bracketed paste applies when the program has mode 2004 on. |

GPUI does the platform work: a native file drag arrives as
`PlatformInput::FileDrop`, becomes an `ExternalPaths` drag, and `Submit` fires
the listener. gpuix itself has no drop handling, so nothing competes.

**Not done:** Option-drag to upload over SSH (iTerm's other drop mode). No
remote sessions exist yet, so there is nothing to upload to.

### 1.6 The data-plane socket test on macOS

Not style, but it blocks `cargo test --workspace` on any Mac. The test's
TempDir used a 19-digit nanosecond suffix; with macOS's 49-byte private
`$TMPDIR` the bound path was 107 bytes against a 104-byte `SUN_LEN`. Now
pid + counter (`st-client-core/src/dataplane.rs`, `TempDir::new`).

---

## 2. What was tried and reverted: matching iTerm's font

The request was "make terminal rendering the same as iTerm". iTerm's profile
(`~/Library/Preferences/com.googlecode.iterm2.plist`) is **Monaco 12**,
spacing 1.0, `Use Bright Bold`, with a separate dark-mode colour set. A config
was written to match it exactly — and verified to match: the native readback
gave `cell {"w":7.201,"h":17.000}`, iTerm's geometry to three decimals.

It looked **worse**, for the two reasons in §1.1: Monaco lacks 11 of the 16
block glyphs, and `line_height = 1.4167` (needed to reach iTerm's 17 px row,
because superterminal's floor excludes leading) opened a 2 px seam between
block rows. Geometry parity was precise and beside the point.

**The lesson worth keeping:** iTerm can use Monaco *because* it draws these
glyphs itself. Now that §1.1 exists, Monaco is usable here too — but only
after §1.1, and it was not when the config was tried. The reverted config is
useful as a reference for iTerm's dark palette; it is reproduced in §5.

**What a user who wants "iTerm-like" should actually set today:** leave the
family on Menlo (or SF Mono / JetBrains Mono — anything with full U+2580
coverage), optionally `size = 12.0`, and consider `line_height = 1.0`, which
lands exactly on the natural line box and is marginally better for block art
than the default `1.2`. With §1.1 in place the line-height choice no longer
affects block tiling at all; it only changes the leading around text.

---

## 3. Facts about the font pipeline that are easy to get wrong

- **Cell height** is `CellSize::for_font`: `max(size × line_height,
  ascent + descent)`. The floor **excludes `line_gap`** (leading). iTerm's
  row is `ceil(ascent + descent + leading) × spacing`, so for a face with
  leading (Monaco: 1.002 px) `line_height = 1.0` here is ~2 px tighter than
  iTerm's 1.0. gpui *does* expose `FontMetrics::line_gap`, but only through
  `TextSystem::read_metrics`, which is private — including leading in the
  floor needs a third gpuix patch. Open question whether that is worth it now
  that block tiling no longer depends on it.
- **`DEFAULT_LINE_HEIGHT` in `props.rs` is `1.0` but the app never uses it.**
  `SurfaceHost` always passes `config.font.lineHeight`, whose schema default is
  `1.2`. The Rust constant only governs a harness-mounted grid. The comment on
  it says so; do not "fix" the mismatch by changing one without the other.
- **`advance('m') / font_size ≈ 0.60` means a real monospace face resolved;
  ≈ 0.83 means it fell through to the proportional UI font.** `DEFAULT_FONT_FAMILY`
  is per-platform (`Menlo` / `Consolas` / `monospace`) for exactly this reason:
  `"monospace"` is a fontconfig alias CoreText cannot resolve.
- **Metric readback exists.** `stReadProp(id, 'cellSize')`,
  `stReadProp(id, 'fontMetrics')` (`ascent`, `descent`, `naturalLineHeight`)
  and `stReadProp(id, 'stats').spriteQuads` answer geometry questions
  exactly. Use them before eyeballing anything.
- **Screen capture needs the terminal process to hold Screen Recording
  permission** on macOS, and the grant does not survive a resumed session. If
  `window-shot.sh` reports `could not create image from window` while the
  window is on screen, that is why — a full-screen `screencapture -x` coming
  back solid black confirms it. Verify by readback or binary inspection
  instead; do not conclude the fix failed.

---

## 4. Still open

| Item | Notes |
|---|---|
| Braille U+2800–U+28FF | Same class of bug (Warp #9696: right-side gap when the font's braille does not fill the cell). Dots are rectangles; a natural extension of `sprites.rs`. |
| Symbols for Legacy Computing U+1FB00–U+1FBFF | Sextants/octants used by some TUI charting. Ghostty and Kitty cover these. |
| Powerline U+E0B0–U+E0B3 | Triangles/arcs; needs paths, not quads. Fonts usually ship them correctly sized, so lower priority. |
| Crosses `╬╪╫` | The strokes run edge to edge, so the centre square is crossed rather than open as in the reference glyph. Invisible at 1 px strokes. |
| Diagonals `╱╲╳` | Left to the font. Would need a path or a rotated quad. |
| Underline / strike on a sprite run | Not painted; a sprite run paints only its geometry. Rare in practice. |
| Leading in the cell floor | See §3. Needs a gpuix patch to read `line_gap`. |
| Daemon theme is read once at start | `[theme]` also feeds OSC 10/11. Changing the file re-themes the cells on the next client launch, but the daemon keeps answering colour queries from its startup config until it restarts. Restarting it kills every live PTY (I1), so this is a documented lag, not a bug to force. |
| Glyph weight vs iTerm | GPUI rasterises text itself; CoreText's smoothing in iTerm is not something a config knob reaches. Not measured. |

---

## 5. Reference: iTerm2 "Default" profile, dark set

For anyone who wants iTerm's *colours* (safe now; the font caveats in §2 no
longer apply after §1.1 either, but Menlo is still the better default here):

```toml
[font]
size = 12.0
line_height = 1.0

[terminal]
bold_is_bright = true

[theme]
foreground = "#DCDCDC"
background = "#15191F"
cursor = "#FFFFFF"
cursor_text = "#000000"
selection_background = "#B3D7FF"
selection_foreground = "#000000"
black = "#14191E"
red = "#B43C2A"
green = "#00C200"
yellow = "#C7C400"
blue = "#2744C7"
magenta = "#C040BE"
cyan = "#00C5C7"
white = "#C7C7C7"
bright_black = "#686868"
bright_red = "#DD7975"
bright_green = "#58E790"
bright_yellow = "#ECE100"
bright_blue = "#A7ABF2"
bright_magenta = "#E17EE1"
bright_cyan = "#60FDFF"
bright_white = "#FFFFFF"
```

Source: `New Bookmarks[0]` in the iTerm plist, the `(Dark)` keys — the
profile has `Use Separate Colors for Light and Dark Mode` on, and its
non-suffixed set is a light scheme that is not what the user sees.

---

## 6. What not to do

- **Do not add a font to fix block art.** The fix is `sprites.rs`; a font
  change only moves the seam.
- **Do not `ceil()` sprite rects** to be safe. See §1.1 — it makes stroke
  weight wobble. Round.
- **Do not put `bg.glass` on a surface that touches the backdrop.** That is
  the exact bug §1.2 fixed. If a new chrome surface sits on the window, it
  gets `bg.chrome`.
- **Do not read only `ansiN` in a new theme consumer.** Accept the documented
  names too, or better, go through `buildTerminalTheme`.
- **Do not trust a headless geometry match as a visual result.** §2 matched
  iTerm's cell to three decimals and looked worse. Check glyph coverage
  (`CTFontGetGlyphsForCharacters`) and look at a block-art logo.
- **Do not restart the daemon to pick up a theme change** in a session with
  live shells. It owns the PTYs.

---

## 7. Verify

```bash
# Geometry and mapping, headless (no window, no GPU needed):
source scripts/env.sh          # Linux without root: the sysroot for fontconfig etc.
cd crates/st-native && cargo +1.97.1 test --lib
#   → 171 passed; sprites::tests::* covers tiling, coverage, arms, doubles, dashes, arcs

# Client-side theme keys:
bun test ./packages/app/src/platform/window-options.test.ts

# Live, in a running client (the app dlopens the addon once; restart it):
pkill -f 'app.tsx'; ./scripts/run.sh --no-build
# then run `claude` or `opencode` in a tab and look at the logo — one solid
# shape, round prompt-box corners — or, if capture is blocked (§3), read
# stReadProp(id, 'stats').spriteQuads (> 0 while a logo is on screen) and
# stReadProp(id, 'cellSize') / 'fontMetrics' to confirm the face and cell.
```

Glyph coverage for any candidate family, the check §2 skipped:

```swift
// swift coverage.swift — prints n/16 for the block shapes the logos use
let f = CTFontCreateWithName("Monaco" as CFString, 12, nil)
var c: [UniChar] = Array("\u{2580}".utf16); var g = [CGGlyph](repeating: 0, count: 1)
print(CTFontGetGlyphsForCharacters(f, &c, &g, 1) && g[0] != 0)  // false for Monaco
```
