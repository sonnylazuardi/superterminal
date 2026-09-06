---
status: accepted
---
# Box Drawing and Block Elements are drawn from the cell box, not shaped through the font

The `<terminal-grid>` element paints U+2500–U+259F (Box Drawing and Block Elements, minus the three diagonals `╱╲╳`) itself, as quads derived from the cell's own width and height (`crates/st-native/src/sprites.rs`). These codepoints never enter the shaped-line cache and never depend on the configured font family.

## Considered options
- **Shape them through the font like every other glyph** (what 1.0 did) — rejected: a glyph fills the font's em box, not the cell. The cell is `max(size × line_height, ascent + descent)`, so with any leading at all a row of `▀▄█` tiles with a seam between rows (0.47 px on Menlo 13 at the default 1.2, a whole device pixel on Retina), and a family that lacks the glyph (Monaco is missing 11 of the 16 block shapes the Claude Code and `opencode` logos use) falls back to a face whose metrics fragment the shape entirely. A pixel-exact geometry match with iTerm's cell was tried and looked worse, because the seam is structural, not a tuning problem.
- **Bundle a font with full coverage and matching metrics** — rejected: it only moves the seam. The font's line box and the terminal's cell are different things, and the user's `line_height` setting moves one but not the other.
- **Draw them ourselves** — accepted. It is what iTerm2 (`iTermBoxDrawingBezierCurveFactory`), Kitty, Ghostty, Alacritty (`builtin_box_drawing`), VTE and xterm.js all do. Alacritty's commit message is the one-line case: the font "tends to overlap or not align, so providing built-in font is the lesser evil."

## Consequences
- A cell that is exactly one sprite codepoint gets a run of its own (`RunSpan.sprite`); sprite runs merge only with sprite runs. Everything else about run grouping is unchanged.
- Sprite edges are **rounded** to device pixels in window space, never `ceil()`ed: two neighbouring cells derive their shared edge from the same grid line, so rounding lands both on the same integer. A rect whose rounded edges coincide is widened to 1 px rather than dropped. Backgrounds keep their `ceil()` because overlap there is invisible.
- `line_height` no longer affects block tiling at all; it only changes the leading around text. Monaco and other families with partial coverage are usable.
- The four arcs `╭╮╯╰` are a `quad()` with a border on two edges and a radius on their corner, so they are real quarter circles. Double-line corners `╔╗╚╝` are hand-listed so outer joins outer and inner joins inner.
- Still open, same class of bug: Braille U+2800–U+28FF, Symbols for Legacy Computing U+1FB00–U+1FBFF, Powerline U+E0B0–U+E0B3. The diagonals stay with the font; they would need a path.
- `stats.spriteQuads` (via `stReadProp(id, 'stats')`) reports how many sprite quads the last frame painted, so a headless harness can confirm the sprite path without a screenshot.
