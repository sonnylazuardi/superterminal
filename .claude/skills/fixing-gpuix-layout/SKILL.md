---
name: fixing-gpuix-layout
description: Diagnose and fix layout, padding, alignment, spacing, centering, overlap and clipping bugs in superterminal's gpuix/GPUI window chrome (title bar, sidebar, tab strip, command palette, toasts) and in the `<terminal-grid>` cell geometry. Use this skill whenever the user reports that the UI looks wrong — "fix the padding", "icons aren't centered", "things overlap", "extra line spacing", "blank/transparent panel", "match this reference screenshot" — or whenever you are about to edit a `style={{...}}` in `packages/app/src/ui/`. gpuix silently ignores whole classes of CSS-looking props, so guessing at styles wastes rebuild cycles; start here instead. Also use it when you need to actually SEE the app window on macOS, since ordinary screenshots miss it.
---

# Fixing gpuix / GPUI layout

## What draws what

Two renderers, and the fix lives in a different place depending on which one owns the pixels:

| You see | Owner | Edit |
|---|---|---|
| Title bar, sidebar, tab rows, palette, banner, toasts | React → `@gpuix/react` → GPUI | `packages/app/src/ui/**`, `packages/app/src/theme/tokens.ts` |
| Terminal cells, cursor, selection, scrollbar | native `<terminal-grid>` (Rust) | `crates/st-native/src/{geometry,paint,runs,props}.rs` |
| Window frame, traffic lights, background | GPUI window options | `packages/app/src/platform/window-options.ts` |

gpuix styles *look* like CSS but are a fixed allowlist mapped onto GPUI/taffy. Anything outside it is dropped without warning, which is why a plausible-looking style change can do nothing at all.

## Check these first

Ordered by how often each one is the actual cause. Every entry here has bitten this codebase for real.

### 1. Missing `display: 'flex'` — flex props are inert without it

`flexDirection`, `alignItems`, `justifyContent` and `gap` do nothing unless the same style object also sets `display: 'flex'`. Children fall back to block stacking.

This is the single highest-yield thing to grep for. It is what made the command palette put each row's shortcut hint on a second line, overlapping the next row:

```tsx
// broken: hint drops below the title
<div style={{ flexDirection: 'row', justifyContent: 'space-between' }}>
// fixed
<div style={{ display: 'flex', flexDirection: 'row', justifyContent: 'space-between' }}>
```

When a container's children stack when they should sit side by side (or vice versa), check this before anything else.

### 2. Missing `minWidth: 0` — text refuses to shrink, so `maxWidth` can't clamp

A flex item's automatic minimum size is its **min-content** width. For a `whiteSpace: 'nowrap'` text run that is the entire string, so the item never shrinks, `maxWidth` loses, `textOverflow: 'ellipsis'` never gets a narrower box to ellipsize into, and the text paints over its neighbour.

Symptom: long titles overlapping the next element, or a close button landing on top of adjacent text.

```tsx
<text style={{ whiteSpace: 'nowrap', textOverflow: 'ellipsis', flexGrow: 1,
               minWidth: 0, overflow: 'hidden', color: tokens.fg.primary }}>
```

Pair it with `overflow: 'hidden'` on the row so nothing paints outside its box, and `flexShrink: 0` on the things that must keep their size (icons, badges, hit targets) so the text is what gives up space.

### 3. Fixed-width columns need `flexShrink: 0`

A `width: 220` sidebar will still be squeezed by a long child unless it also sets `flexShrink: 0`. Give the column `flexShrink: 0` and let the content ellipsize inside it.

### 4. A glyph is not centered by `alignItems: 'center'`

Centering the *text element* is not centering the *glyph inside its line box*. A font with asymmetric ascent/descent, or an advance wider than its ink, still renders high, low or off to one side.

Give the glyph a deterministic box instead — this is what `Glyph` in `packages/app/src/ui/Icon.tsx` does, so prefer reusing it:

```tsx
<text style={{ width: '100%', textAlign: 'center', lineHeight: box, color, fontSize }}>
```

`lineHeight` equal to the button's side fixes it vertically; `width: '100%'` + `textAlign: 'center'` fixes it horizontally. The parent still needs `display: 'flex'` for `width: '100%'` to mean the button.

### 5. Exotic glyphs shape from a fallback font

Chrome text renders in the system UI font. A character it doesn't cover is resolved through a fallback face with unrelated metrics, so a row of icons ends up on several different baselines and sizes. Fullwidth forms (`＋` U+FF0B), dingbats (`❯` U+276F, `❏` U+274F) and geometric shapes (`▤` U+25A4) are the usual offenders.

Stick to ASCII, Latin-1 and common punctuation — `+`, `×`, `›`, `≡`, `⋯`, `□`. `ICONS` in `Icon.tsx` is the vetted set; add there rather than inlining a new character.

### 6. Every `<text>` must set `color`

GPUI does not inherit color (repo invariant I10). A `<text>` without one is a bug even if it happens to look right.

### 7. A click on a child also fires the ancestor's `onClick`

gpuix attaches `on_click` per element and never calls `cx.stop_propagation()` (`renderer.rs`, the `"click" =>` arm), and the event it emits to JS carries no way to stop propagation. So a button nested inside a clickable row triggers BOTH handlers.

Symptom: clicking a tab's ✕ closed the tab and then errored with `tab 19 does not exist`, because the row's activate handler fired for the tab that had just been deleted.

Fix it structurally rather than with a flag or a propagation hack — the outer element carries layout only, and the clickable area becomes a sibling of the button:

```tsx
<div style={{ display: 'flex', ... }}>                            {/* no onClick */}
  <div onClick={activate} style={{ display: 'flex', flexGrow: 1, minWidth: 0 }}>…</div>
  <div onClick={close}>×</div>
</div>
```

### 8. Alpha overlays over a terminal

A panel at 80% alpha lets the terminal's own text read straight through it. Panels that must be readable need to be near-opaque — `tokens.bg.overlay`.

## Is the prop even supported?

Two greps, because *accepted* and *applied* are different things. `line_height` was accepted by the style type and silently ignored for a while.

```bash
grep -n '<prop_in_snake_case>' vendor/gpuix/packages/native/src/style.rs     # accepted?
grep -n '<prop_in_snake_case>' vendor/gpuix/packages/native/src/renderer.rs  # actually applied?
```

Known-good: `display`, `flexDirection`, `flexGrow`, `flexShrink`, `alignItems`, `justifyContent`, `gap`, `width`/`height`/`minWidth`/`maxWidth`, `padding*`/`margin*`, `border*Width`, `borderColor`, `borderRadius`, `backgroundColor`, `overflow`/`overflowX`/`overflowY` (`hidden`/`scroll`), `whiteSpace`, `textOverflow: 'ellipsis'`, `textAlign`, `lineHeight`, `hover: {...}`, `cursor`.

Note `overflow: 'hidden'` maps to GPUI's `overflow_hidden()` only when both axes resolve to hidden; otherwise it takes the axis-specific path.

## See your work — the verification loop

Do not fix layout blind, and do not ask the user to describe it. Look at it.

```bash
.claude/skills/fixing-gpuix-layout/scripts/window-shot.sh /tmp/win.png            # whole window
.claude/skills/fixing-gpuix-layout/scripts/window-shot.sh /tmp/hdr.png 0 0 230 46 # a region of it
```

Then Read the PNG. Regions are captured at Retina 2x, which is what makes a few points of misalignment visible and measurable.

Three traps that waste time here:

- **A plain full-screen `screencapture` usually misses the window** — it sits behind whatever else is open. The script resolves the window's real id and bounds through CoreGraphics instead.
- **AppleScript / System Events is not permitted** on this machine (no assistive access). Don't reach for `osascript` to find or front the window.
- **`sips --cropOffset` crops from the CENTRE, not the top-left.** Capture a region with the script rather than cropping afterwards. `sips -Z <max> in.png --out out.png` is fine for scaling down to view.

To relaunch after an edit:

```bash
pkill -f 'app.tsx'; sleep 1
env -u NAPI_RS_NATIVE_LIBRARY_PATH -u SUPERTERMINAL_SOCKET \
  nohup ./scripts/run.sh --no-build > /tmp/st-ui.log 2>&1 &
sleep 8; grep -icE 'invalid hook|Unknown element|TypeError|panicked' /tmp/st-ui.log
```

Chrome-only edits need just this relaunch. Editing `crates/st-native` needs a rebuild first (see below).

## Measure, don't estimate

The native element publishes its real geometry, so alignment and cell-size questions have exact answers:

```js
native.stReadProp(surfaceId, 'cellSize')  // {w, h} — the actual cell box
native.stReadProp(surfaceId, 'size')      // {cols, rows}
native.stListGrids()                      // surfaces with a mounted grid
native.stReadableProps()                  // everything readable
```

Get the addon with `createRequire(import.meta.url)(process.env.NAPI_RS_NATIVE_LIBRARY_PATH)`.

A throwaway harness **must live under `packages/app/`** or `@gpuix/react` won't resolve. Delete it when done. Wrap the grid in a full-size flex column, or it collapses to one row and you'll measure the collapse instead of the thing you care about:

```tsx
<div style={{ display: 'flex', flexDirection: 'column', width: '100%', height: '100%' }}>
  <div style={{ flexGrow: 1, display: 'flex' }}>
    <terminal-grid surfaceId={1} socketPath={process.env.SUPERTERMINAL_SOCKET}
                   fontSize={14} lineHeight={1.2} style={{ flexGrow: 1 }} />
  </div>
</div>
```

`DEBUG=st:keys` traces key events reaching the React root and what command they matched — use it when a shortcut "does nothing", to tell "never arrived" apart from "matched nothing".

## Terminal cell geometry

Cell width is the advance of `'m'` in the resolved font; cell height is `font_size × line_height` (`crates/st-native/src/geometry.rs`, `CellSize`). Defaults live in two places that must agree in spirit:

- `crates/st-native/src/props.rs` — `DEFAULT_FONT_FAMILY` (per-platform; `"monospace"` is a CSS generic that CoreText cannot resolve, and falling through to the proportional system font makes glyphs drift inside their cells), `DEFAULT_FONT_SIZE`, `DEFAULT_LINE_HEIGHT`.
- `packages/app/src/config/schema.ts` + `docs/config-example.toml` — the user-facing defaults, asserted by `packages/app/src/config/load.test.ts`. Change all three together or that test fails.

A quick sanity check on the resolved font: `cellSize.w / fontSize` ≈ 0.60 for Menlo, ≈ 0.83 if you've accidentally landed on the proportional UI font.

Rebuild after editing the crate — the `+1.97.1` is required, because rustup resolves `rust-toolchain.toml` from the invocation directory and `vendor/gpuix` is not an ancestor of `crates/st-native`:

```bash
source scripts/env.sh
(cd crates/st-native && CARGO_PROFILE_RELEASE_DEBUG=0 cargo +1.97.1 build --release)
cp crates/st-native/target/release/libst_native.dylib \
   packages/native/superterminal-native.darwin-arm64.node
```

~1 minute incremental. The app `dlopen`s the addon once per process, so a rebuild always needs a full app restart.

## macOS window chrome

`titlebarTransparent` is set on macOS, so the app draws under the system traffic lights and must inset for them itself. The geometry is arithmetic, not taste: `trafficLightX`/`trafficLightY` position the group's **top-left**, and three 12pt buttons on a 20pt pitch from x=18 put the last button's right edge at **x=70**. So the left inset must exceed 70 (`tokens.padding.trafficLights`), and centering them in a title bar of height `h` means `trafficLightY = (h - 12) / 2`.

## Two things that will burn a cycle

- **Never call `renderer.simulateClick` on macOS.** It aborts the process with `cannot update GpuixView while it is already being updated` — `patches/0002` fixes only the Linux mouse path. Use `renderer.focusElement(id)` then `renderer.simulateKeyDown('<chord>')`.
- **Never run the client under `bun --hot`.** Re-evaluated component modules bind a second copy of React while `globalThis.__stRoot` still holds the old reconciler, and the first edit blanks the window with `Invalid hook call` / `null is not an object (evaluating 'dispatcher.useContext')`. Plain `bun`, and restart — the terminals live in the daemon (invariant I1), so a restart is cheap and loses nothing.

## Finishing

- Derive spacing from `tokens.space` / `tokens.strip` rather than hardcoding numbers. When two things must align, compute the relationship (see `sidebarIconInset` in `Icon.tsx`) so it can't drift when a token changes.
- `bun run typecheck` and `bun test` both pass. Editor diagnostics can lag a multi-file edit — trust a fresh `bun run typecheck` exit code over a stale squiggle.
- Respect invariant I4: no status bar. Chrome carries window affordances, not terminal state.
- Comment the *why*, especially for a trap above. A bare `minWidth: 0` reads as noise; the next person needs to know it's load-bearing.
