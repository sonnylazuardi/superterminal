# Handover: gpuix chrome UI (vertical tabs + layout skill)

Implementation brief for an agent working from **GitHub `main`** of
[`sonnylazuardi/superterminal`](https://github.com/sonnylazuardi/superterminal).
Recreate only the **window chrome** work that currently lives as uncommitted
local changes against `33e5d34` (`Make the app actually launch…`).

Do **not** port native-crate, packaging, `scripts/env.sh`, `scripts/run.sh`,
server-path, or gpuix custom-element-registration work. Those are a different
handover.

---

## 0. Baseline and goal

| | |
|---|---|
| Start from | `origin/main` @ `33e5d34` |
| End state | Vertical-tab sidebar is the default chrome; horizontal strip still works via toggle; command palette / toasts / banner / icons actually lay out; a skill exists so the next UI edit is not guesswork |
| Runtime | gpuix/GPUI via `@gpuix/react`. Styles *look* like CSS but are a **fixed allowlist**. Unsupported props are dropped with no warning. |

On origin, vertical tabs already exist as a flag (`ui.verticalTabs`, command
`view.toggleVerticalTabs`, default **false**), but the layout is a thin
variant of the horizontal strip: an empty 58px `TitleBarSpacer` across the
whole window, then a 220px `TabStrip` column with the same chip/tab/＋
children. This work replaces that with a real sidebar chrome.

**Done when:**

- Fresh launch on macOS shows a **left sidebar** (not a top tab strip).
- Sidebar header sits beside the traffic lights: `≡` (toggle) on the left, `+`
  (new tab) on the right. No overlap with the green button.
- Sidebar lists a session **section header** then tab **rows** with a leading
  `›` chip, title (ellipsized), and `×` close. Many tabs **scroll** inside the
  sidebar.
- Footer: `⋯` (palette) left, `□` (new session) right.
- Content column has its own header (`□` + active surface title) above the
  grid, same height as the sidebar header, split by a 1px full-height divider.
- `≡` toggles to the old horizontal strip (real title bar, not a spacer).
- ⌘K / ⌘⇧P palette appears **top-center**, opaque, one row per command with
  the shortcut on the **same line**.
- Toasts stack **bottom-right**.
- Clicking a tab’s `×` closes **only** that tab (no `tab N does not exist`).
- `bun run typecheck` and `bun test` pass.

---

## 1. Files to create

```
.claude/skills/fixing-gpuix-layout/SKILL.md
.claude/skills/fixing-gpuix-layout/scripts/window-shot.sh   # chmod +x
packages/app/src/ui/Icon.tsx
```

Full contents for all three are in **§9**. Copy them verbatim. The skill is
not documentation-for-humans — it is the procedure for **every** subsequent
`style={{…}}` edit in `packages/app/src/ui/`. After creating it, **read it
before touching chrome**.

## 2. Files to modify (chrome only)

| File | What changes |
|---|---|
| `packages/app/src/theme/tokens.ts` | Expand the token scale. **Never inline hex / magic px in components after this.** |
| `packages/app/src/ui/App.tsx` | Replace `TitleBarSpacer` + shared frame with split sidebar / content columns. |
| `packages/app/src/ui/TabStrip.tsx` | Sidebar is a scrolling list of rows; click targets become siblings; glyphs via `Icon.tsx`. |
| `packages/app/src/ui/Banner.tsx` | `display:'flex'`; tokens not raw numbers; toasts need `side`/`align` + opaque panel. |
| `packages/app/src/ui/CommandPalette.tsx` | Same flex + anchored-placement traps; overlay alpha; row ellipsis. |
| `packages/app/src/ui/SurfaceHost.tsx` | Centre the empty state; `focused={!paletteOpen}` on `<terminal-grid>`. |
| `packages/app/src/platform/window-options.ts` | `trafficLightY: 13` (was 18). |
| `packages/app/src/platform/window-options.test.ts` | Assert `trafficLightY: 13`. |
| `packages/app/src/config/schema.ts` | `verticalTabs` default **`true`**. |
| `packages/app/src/config/load.test.ts` | `DEFAULT_CONFIG.window.verticalTabs === true`. |
| `docs/config-example.toml` | `vertical_tabs = true` plus the comment in §7. |

`useRunCommand` in `ui/context.tsx` already exists on origin — reuse it.
`view.toggleVerticalTabs` already exists — do not add a new command.

---

## 3. Layout trees

### Origin (`33e5d34`)

```
<App>
 ├─ <TitleBarSpacer/>     macOS only: empty height tokens.padding.trafficLights (58)
 ├─ <Banner/>
 ├─ <Frame row|column>    flexDirection from ui.verticalTabs
 │   ├─ <TabStrip/>       owns width 220 / height 36, glass bg, border
 │   └─ <SurfaceHost/>
 ├─ <CommandPalette/>
 └─ <StatusToasts/>
```

### Target — vertical (default)

```
<App>                                         column, 100%×100%, transparent
 ├─ <Banner/>                                 only when disconnected
 ├─ <Frame testId="frame">                    row, flexGrow 1, overflow hidden
 │   ├─ sidebar testId="sidebar"              column, width verticalWidth, flexShrink 0
 │   │   ├─ <SidebarHeader/>                  height titleBarHeight; ≡ … +
 │   │   ├─ <TabStrip/>                       flexGrow 1, overflowY scroll
 │   │   └─ <SidebarFooter/>                  height footerHeight; ⋯ … □
 │   ├─ divider testId="sidebar-divider"      width border.width, full height
 │   └─ content testId="content"              column, flexGrow 1, minWidth 0, overflow hidden
 │       ├─ <ContentHeader/>                  height titleBarHeight; □ + title
 │       └─ <SurfaceHost/>
 ├─ <CommandPalette/>
 └─ <StatusToasts/>
```

There is **no** full-width title bar and **no** `TitleBarSpacer` in this mode.
The sidebar column **is** the traffic-light row. Delete `TitleBarSpacer`.

### Target — horizontal (`ui.verticalTabs === false`)

```
<App>
 ├─ <TitleBar testId="titlebar"/>             full width; ≡ + title + +
 ├─ <Banner/>
 ├─ <Frame testId="frame">                    column, flexGrow 1, overflow hidden
 │   ├─ <TabStrip/>                           height 36, glass + bottom border
 │   └─ <SurfaceHost/>
 ├─ <CommandPalette/>
 └─ <StatusToasts/>
```

Toggle: `run('view.toggleVerticalTabs')` from `≡` in `SidebarHeader` /
`TitleBar`. Config `window.verticalTabs` (default true) is applied at
bootstrap the same way origin already does.

---

## 4. Token contract

Replace the origin `Tokens` shape. Components after this pick **only** from
`tokens.space` / `tokens.strip` / `tokens.bg` / `tokens.radius` / `tokens.font`.

```ts
export interface Tokens {
  bg: {
    glass: string;
    glassHover: string;
    glassActive: string;
    /** Chips inside an already-glass surface (row icons); softer than `glass`. */
    glassSubtle: string;
    overlay: string;
  };
  border: { glass: string; width: number };
  fg: { primary: string; muted: string; danger: string };
  accent: string;
  radius: { panel: number; tab: number; chip: number; chipSmall: number };
  font: { chrome: number; chip: number; paletteInput: number; family?: string };
  space: { xs: number; sm: number; md: number; lg: number; xl: number };
  padding: { trafficLights: number };
  strip: {
    height: number;
    titleBarHeight: number;
    footerHeight: number;
    iconButton: number;
    sidebarPadding: number;
    sidebarSectionLabel: number;
    sectionHeaderHeight: number;
    rowIcon: number;
    rowPaddingX: number;
    rowHeight: number;
    paletteInputHeight: number;
    chipHeight: number;
    renameWidth: number;
    tabHeight: number;
    toastMaxWidth: number;
    tabMaxWidth: number;
    tabMinWidth: number;
    gap: number;
    paddingX: number;
    verticalWidth: number;
  };
}
```

Exact values (keep these; they were measured off the rendered window):

```ts
const SPACE = { xs: 2, sm: 4, md: 6, lg: 8, xl: 12 } as const;

// Pixel-read of the traffic-light group with trafficLightX: 18.
// Buttons are 14pt across on a 23pt pitch: x 18..32, 41..55, 64..78.
// Group ends at 78, NOT the textbook 70 (3×12 on a 20pt pitch).
const TRAFFIC_LIGHTS_RIGHT = 78;

const shared = {
  fg: { primary: '#F2F2F2', muted: '#FFFFFF80', danger: '#FF6B6B' },
  accent: '#7AA2F7',
  radius: { panel: 16, tab: 8, chip: 999, chipSmall: 6 },
  font: { chrome: 12.5, chip: 11.5, paletteInput: 14 },
  space: SPACE,
  padding: { trafficLights: TRAFFIC_LIGHTS_RIGHT + SPACE.lg }, // 86
  strip: {
    height: 36,
    titleBarHeight: 38,       // buttons sit y 13..27; 38 → symmetric band
    footerHeight: 32,
    iconButton: 22,
    sidebarPadding: 8,
    sidebarSectionLabel: 11,
    sectionHeaderHeight: 24,
    rowIcon: 18,
    rowPaddingX: 8,
    rowHeight: 30,
    paletteInputHeight: 32,
    chipHeight: 22,
    renameWidth: 140,
    tabHeight: 28,
    toastMaxWidth: 320,
    tabMaxWidth: 220,
    tabMinWidth: 90,
    gap: 4,
    paddingX: 12,
    verticalWidth: 220,
  },
} as const;
```

Palette deltas vs origin:

| Token | Origin | Target | Why |
|---|---|---|---|
| `bg.glassHover` | `#FFFFFF1A` | `#FFFFFF14` | Quieter hover on glass |
| `bg.glassSubtle` | *(missing)* | `#FFFFFF14` | Leading icon chip on a glass sidebar |
| `bg.overlay` (glass) | `#16161ECC` (80%) | `#16161EFA` | Terminal text was reading through the palette |
| `padding.trafficLights` | `58` | `86` | Hover fill of `≡` was on top of the green button |
| opaque `glassSubtle` | — | `#24242A` | Linux opaque sibling of `glassSubtle` |

`buildTerminalTheme` / `DEFAULT_TERMINAL_THEME` / `tokensFor()` stay as on
origin.

---

## 5. `App.tsx` — implement these components

Root `onKeyDown` is origin’s, plus `DEBUG=st:keys` logging (optional but
useful). Palette still swallows `escape`/`up`/`down`/`enter`.

### `AppFrame`

- `vertical ? null : <TitleBar />` **above** `<Banner />`.
- Then the vertical or horizontal frame from §3.
- Vertical sidebar `backgroundColor: tokens.bg.glass`.
- Vertical divider `backgroundColor: tokens.border.glass`.
- Content `minWidth: 0` + `overflow: 'hidden'` or a long title widens the
  window.
- Sidebar `flexShrink: 0` or a long tab title **squeezes the 220px column**.

### `SidebarHeader`

Height `tokens.strip.titleBarHeight`, row, `alignItems: 'center'`,
`flexShrink: 0`, `gap: tokens.space.xs`.

```
paddingLeft:  platform.isMac ? tokens.padding.trafficLights : sidebarIconInset(tokens)
paddingRight: sidebarIconInset(tokens)
```

Children: `IconButton` `≡` (`testId="sidebar-toggle"`,
`view.toggleVerticalTabs`) → spacer `flexGrow: 1` → `IconButton` `+`
(`testId="new-tab"`, `tab.new`).

`sidebarIconInset` lives in `Icon.tsx`. It lines the header/footer **glyph
centres** up with the tab-row leading icons:

```
sidebarPadding + rowPaddingX + (rowIcon - iconButton) / 2
```

Without it the footer icons sit ~6pt left of the row icons.

### `SidebarFooter`

Height `footerHeight`, same inset both sides, top border
(`borderTopWidth` + `borderColor: tokens.border.glass`). `⋯` →
`palette.commands` (`testId="sidebar-palette"`); `□` → `session.new`
(`testId="sidebar-new-session"`).

Invariant **I4**: chrome carries window affordances, **not** terminal state.
No cwd / exit code / shell in this footer.

### `ContentHeader`

Same height as the sidebar header. `paddingLeft/Right: rowPaddingX` so the
`□` sits over the grid’s first column (`SurfaceHost` already pads the grid
`left: 8`, which is `rowPaddingX`). Leading 18×18 `Glyph` `□`, then the
active surface title (`selectActiveSurface`) with `minWidth: 0`,
`overflow: 'hidden'`, `whiteSpace: 'nowrap'`, `textOverflow: 'ellipsis'`.
Fallback title `'superterminal'`. `testId="content-header"`.

### `TitleBar` (horizontal only)

Replaces `TitleBarSpacer`. Glass + bottom border. Mac left pad =
`trafficLights`, else `sidebarPadding`. `≡` + ellipsizing title + `+`.
`testId="titlebar"`.

---

## 6. `TabStrip.tsx`

The strip **no longer owns** the sidebar’s width, background, or right
border. `App.tsx` owns the column. In vertical mode the strip is just the
scrolling list.

### Container

Vertical:

- `flexDirection: 'column'`, `alignItems: 'stretch'`
- **No `gap`** — rows use `marginBottom: space.xs`. A flex `gap` *plus*
  that margin doubled the spacing.
- `flexGrow: 1`, `minHeight: 0`, `overflowY: 'scroll'`
- `paddingLeft/Right/Bottom: sidebarPadding` (no top pad; the section
  header supplies `marginTop`)

Horizontal: keep origin’s row, `gap`, height 36, `paddingX`, glass + bottom
border, `overflow: 'hidden'`.

### New-tab button

Vertical: **omit it**. `+` lives in `SidebarHeader` (always visible). A
second one at the end of a scrolling list drifts off-screen.

Horizontal: keep `NewTabButton`, sized `iconButton` not 24, glyph `+` via
`Glyph` (not fullwidth `＋`).

### `SessionChip`

**Vertical** is a section header, not a pill:

- `display: 'flex'` + `alignItems: 'center'` (without `display:'flex'`,
  `alignItems` is inert and the label sits at the **top** of the 24pt band)
- height `sectionHeaderHeight`, pad `rowPaddingX` both sides so the label’s
  left edge = the row icons’ left edge
- `marginTop: space.sm`, muted `sidebarSectionLabel` size, ellipsis +
  `minWidth: 0`
- still `onClick` → `session.switch`
- `testId="session-chip"`

**Horizontal** stays a pill, but add `display: 'flex'` (same trap). Height
`chipHeight`, pad `space.xl`, `marginRight: space.sm`.

**Rename `<input>`:** width `vertical ? '100%' : renameWidth`, height
`chipHeight` (must match the chip or the strip jumps). A 140px island under
full-width rows looks like a bug.

### `Tab` — two load-bearing structural changes

**1. Click targets are siblings.** gpuix attaches `on_click` per element and
**never** `cx.stop_propagation()`. A nested `×` inside a clickable row fires
**both** handlers: the tab closes, then `tab.set_active` runs for the deleted
id → toast `tab N does not exist`.

```
<div testId={`tab-${id}`}>                    {/* LAYOUT ONLY — no onClick */}
  <div testId={`tab-${id}-activate`} onClick={onActivate} style={{ flexGrow:1, minWidth:0, overflow:'hidden', display:'flex', ... }}>
    … bell, leading icon, title, badge, Close?
  </div>
  <div testId={`tab-${id}-close`} onClick={onClose}> × </div>
</div>
```

Repeat `minWidth: 0` + `overflow: 'hidden'` on the activate wrapper — it is
now the flex item the title shrinks inside.

**2. Vertical row chrome**

- height `rowHeight`, `width: '100%'`, `flexShrink: 0`,
  `marginBottom: space.xs`, `overflow: 'hidden'`
- pad `rowPaddingX` both sides (symmetric so `×` mirrors the leading icon)
- always draw `borderWidth: 1`: `border.glass` when active, `'transparent'`
  otherwise, so rows **do not shift 1px** on activate
- active fill `glassActive`; hover `glassHover` (or `glassActive` if already
  active)
- leading 18×18 chip (`borderRadius: chipSmall`): `glassSubtle` idle,
  `glassActive` when selected; `Glyph` `›` (`ICONS.chevron`), accent when
  active else muted
- title: `whiteSpace: 'nowrap'` + ellipsis + `minWidth: 0` in **both**
  orientations (drop origin’s vertical `lineClamp: 2` / `whiteSpace: 'normal'`)
- close: 18×18, `flexShrink: 0`, `Glyph` `×` (`ICONS.close`) — **not** dingbat `✕`

**Horizontal** additionally: `minWidth: tabMinWidth`, `maxWidth: tabMaxWidth`,
`flexShrink: 1`. Origin only set `maxWidth`. A nowrap title’s automatic
min-content is the **whole string**, so `maxWidth` lost and the close button
landed on the next tab.

Surface the real error on activate:

```ts
.catch((err) => toast `Could not switch tab: ${err.message}`)
```

Origin swallowed it as `'Could not switch tab'`.

---

## 7. Other chrome

### `Banner.tsx`

Add `display: 'flex'` on the banner row (origin set `flexDirection` without
it → action button wrapped under the message). Spacing from `tokens.space`.
Message: `flexGrow: 1`, `minWidth: 0`, `overflow: 'hidden'`. Action:
`flexShrink: 0` + `display: 'flex'`.

### `StatusToasts` (same file)

`<anchored>` placement is two knobs:

- `side` / `align` pick the **trigger point** on the parent
  (`custom_elements/anchored.rs`, `wrap_at_trigger`)
- `anchor` picks **which corner of the layer** lands on that point

Both default to bottom/start = parent’s **bottom-left**. Origin’s
`anchor="bottomRight"` alone put the stack’s right edge on the left window
edge; `snapToWindow` then dragged it to x=8. Fix:

```tsx
<anchored
  testId="toasts"
  side="bottom"
  align="end"
  anchor="bottomRight"
  offset={{ x: -tokens.space.xl, y: -tokens.space.xl }}
  style={{
    display: 'flex',
    flexDirection: 'column',
    gap: tokens.space.xs,
    backgroundColor: tokens.bg.overlay,
    borderRadius: tokens.radius.tab,
    borderWidth: tokens.border.width,
    borderColor: tokens.border.glass,
    padding: tokens.space.sm,
    maxWidth: tokens.strip.toastMaxWidth,
  }}
>
```

An `<anchored>` **cannot** be transparent: the element forces opaque `#1A1A1A`
when the layer’s background resolves to alpha 0. Give the **stack** the panel
surface and keep inner toasts as plain rows. Otherwise the forced grey shows
in the gaps.

Toast text: **no** `whiteSpace: 'nowrap'`; `minWidth: 0` so long server errors
wrap inside `toastMaxWidth` instead of spanning the window. Click still
dismisses.

### `CommandPalette.tsx`

Same placement trap. Origin `anchor="topCenter"` + `offset.y: 80` hung the
panel off the bottom edge; snap dragged it to bottom-left, clipped.

```tsx
<anchored
  testId="command-palette"
  side="bottom"
  align="center"
  anchor="topCenter"
  offset={{ x: 0, y: tokens.strip.titleBarHeight + tokens.space.xl }}
  style={{
    width: 560,
    display: 'flex',           // REQUIRED or children are blocks
    flexDirection: 'column',
    backgroundColor: tokens.bg.overlay,
    borderRadius: tokens.radius.panel,
    borderWidth: tokens.border.width,
    borderColor: tokens.border.glass,
    overflow: 'hidden',        // keep rounded corners from being painted over
    padding: tokens.space.lg,
    gap: tokens.space.xs,
  }}
>
```

Input: height `paletteInputHeight`, pad `space.lg` (same as a row, so query
and titles share a left edge), `marginBottom: space.md`.

Each row: `display: 'flex'`, `flexDirection: 'row'`, height `rowHeight`, pad
`space.lg`, `gap: space.lg`. Title `flexGrow: 1` + `minWidth: 0` + nowrap
ellipsis; hint `flexShrink: 0`. Origin omitted `display: 'flex'` → hint
dropped onto a **second line** and overlapped the next row.

Empty state: wrap the `<text>` in a flex row of height `rowHeight` (a bare
`<text>` with padding does not centre).

Logic (filter, session rows, New Session trailing row, MAX_ROWS = 8 window)
stays as origin.

### `SurfaceHost.tsx`

Empty state: add `display: 'flex'` so `alignItems`/`justifyContent` actually
centre “No open tabs”.

On `<terminal-grid>`, add:

```tsx
focused={!paletteOpen}
```

Switching tabs remounts the element (`key={surface.id}`). Nothing else
refocuses the replacement. GPUI delivers keys along the focus chain, so an
unfocused grid means an **empty** chain: neither the element nor the root
`onKeyDown` fires, and ⌘1…⌘9 / ⌘T / ⌘W die until the user clicks the
terminal. Gate on the palette so its `<input>` can hold focus; closing the
palette flips `false → true` and the element acts on the change (`props.rs`
`"focused"`).

Read `paletteOpen` from `s.ui.paletteOpen`. Padding stays
`{ top: 4, right: 8, bottom: 4, left: 8 }`.

### Window options

```ts
// packages/app/src/platform/window-options.ts  (macOS branch)
{ trafficLightX: 18, trafficLightY: 13 }
```

`trafficLightY` is the group’s **top** inset. 18 left the buttons ~5pt below
the sidebar header icons. Pixel read: buttons are 14pt tall (not 12), span
y 13..27, centre at 20 — the same place the `≡`/`+` ink centres land in their
22pt boxes inside the 38pt bar. Do **not** “correct” this to
`(38 - 12) / 2 = 13` by coincidence with a 12pt assumption, and do **not**
use 12.

Update `window-options.test.ts`: `trafficLightY: 13`. The test title can
keep saying `18/18` or be renamed; the assertion is what matters.

### Config default

```ts
// packages/app/src/config/schema.ts
verticalTabs: z.boolean().default(true),
```

```ts
// packages/app/src/config/load.test.ts
expect(DEFAULT_CONFIG.window).toEqual({ background: 'auto', verticalTabs: true });
```

```toml
# docs/config-example.toml  [window]
# Put the tab strip down the left edge instead of along the top. Default since
# the macOS bring-up: a sidebar keeps full shell titles readable, where the
# horizontal strip has to ellipsize them after about three tabs. Set false for
# the horizontal strip; the title bar's leftmost button toggles it at runtime.
vertical_tabs = true
```

---

## 8. gpuix traps (must follow; this is why the skill exists)

These have all bitten this chrome for real. Full write-up is the skill in §9.

1. **`display: 'flex'` is required.** `flexDirection` / `alignItems` /
   `justifyContent` / `gap` are inert without it. Children fall back to
   block stacking.
2. **`minWidth: 0`** on any flex text that must ellipsize. Automatic minimum
   is min-content = the whole nowrap string.
3. **Fixed-width columns need `flexShrink: 0`.**
4. **A glyph is not centred by `alignItems: 'center'`.** Use `Glyph`:
   `width: '100%'` + `textAlign: 'center'` + `lineHeight: box`.
5. **Only ASCII / Latin-1 / common punctuation.** Chrome text is the system
   UI font. Fullwidth `＋`, dingbats `✕` `❯` `❏`, geometric `▤` shape from
   fallback faces with unrelated metrics. Use `ICONS` in `Icon.tsx`.
6. **Every `<text>` sets `color`.** GPUI does not inherit it (invariant I10).
7. **Nested `onClick` fires both handlers.** Sibling hit targets, not
   `stopPropagation` (gpuix has none).
8. **`tokens.bg.overlay` is near-opaque.** 80% alpha lets terminal cells
   read through the palette.
9. **`<anchored>` needs `side` + `align` + `anchor`.** `anchor` alone is the
   layer corner, not the trigger point.
10. **Before shipping a style prop, grep both**
    `vendor/gpuix/packages/native/src/style.rs` (accepted?) **and**
    `renderer.rs` (applied?). `lineHeight` is applied in current vendor.

**Do not:**

- Call `renderer.simulateClick` on macOS (process abort).
- Run the client under `bun --hot` (`Invalid hook call`, blank window).
- Hardcode spacing numbers once tokens exist.
- Put a status bar in the footer (I4).

### Verification loop (macOS)

Chrome-only edits: relaunch, do not rebuild native.

```bash
pkill -f 'app.tsx'; sleep 1
env -u NAPI_RS_NATIVE_LIBRARY_PATH -u SUPERTERMINAL_SOCKET \
  nohup ./scripts/run.sh --no-build > /tmp/st-ui.log 2>&1 &
sleep 8; grep -icE 'invalid hook|Unknown element|TypeError|panicked' /tmp/st-ui.log
```

Then screenshot the **window** (plain `screencapture` usually misses it):

```bash
.claude/skills/fixing-gpuix-layout/scripts/window-shot.sh /tmp/win.png
.claude/skills/fixing-gpuix-layout/scripts/window-shot.sh /tmp/hdr.png 0 0 230 46
```

`Read` the PNG. Captures are Retina 2x. AppleScript is not available (no
assistive access). Do not crop with `sips --cropOffset` (it crops from the
centre).

Checklist to exercise, not just screenshot:

- [ ] Launch → vertical sidebar, traffic lights clear of `≡`
- [ ] `+` opens a tab; row appears; title ellipsizes when long
- [ ] Click row activates; click `×` closes **without** a toast
- [ ] Enough tabs to scroll the sidebar; header/footer stay put
- [ ] `≡` → horizontal title bar + top strip; `≡` back
- [ ] ⌘⇧P palette top-center, hint on the same line as the title
- [ ] ⌘K session mode
- [ ] After switching tabs, ⌘T still works without clicking the grid
- [ ] Force a toast (e.g. close last-but-fail) → bottom-right, wraps
- [ ] `bun run typecheck` and `bun test`

---

## 9. New files — copy verbatim

### 9.1 `packages/app/src/ui/Icon.tsx`

```tsx
/**
 * Chrome icons.
 *
 * Its own module rather than a helper inside `App.tsx`, because `App` imports
 * `TabStrip` — the reverse import would be a cycle.
 *
 * # Why glyphs are centred explicitly
 *
 * `alignItems: 'center'` centres the text ELEMENT inside the button, not the
 * glyph inside its line box. A glyph whose font's ascent/descent are asymmetric
 * (or whose advance is wider than its ink) still sits high, low or off to one
 * side, which is exactly what the first pass looked like. So every icon gets a
 * deterministic box instead: `width: '100%'` + `textAlign: 'center'` fixes it
 * horizontally, and `lineHeight` equal to the button side fixes it vertically —
 * both are real gpuix style props (`style.rs` `text_align` / `line_height`).
 *
 * # Why these particular characters
 *
 * Chrome text renders in the system UI font, and a character it does not cover
 * is resolved through a fallback face with unrelated metrics — the reason the
 * first pass mixed a fullwidth `＋` (U+FF0B), dingbats (`❯` U+276F, `❏` U+274F)
 * and geometric shapes (`▤` U+25A4) and got four different visual baselines.
 * Everything below is ASCII, Latin-1 or common punctuation that SF Pro covers,
 * so they all shape from one face.
 */

import type { Tokens } from '../theme/tokens.js';

export const ICONS = {
  /** Toggle the sidebar / tab orientation. */
  sidebar: '≡', // ≡ IDENTICAL TO
  /** New tab. ASCII '+', not the fullwidth form. */
  newTab: '+',
  /** Command palette. */
  palette: '⋯', // ⋯ MIDLINE HORIZONTAL ELLIPSIS
  /** New session. */
  newSession: '□', // □ WHITE SQUARE
  /** Leading marker on a sidebar tab row. */
  chevron: '›', // › SINGLE RIGHT-POINTING ANGLE QUOTATION MARK
  /** Close a tab. Latin-1 multiplication sign, not a dingbat cross. */
  close: '×', // ×
  /** The active Surface, in the content header. */
  surface: '□', // □
} as const;

/**
 * Left/right inset for the sidebar's header and footer icon buttons.
 *
 * Derived, not eyeballed, so the buttons' glyph centres land on the SAME
 * vertical line as the tab rows' leading icons. A row's icon centre sits at
 * `sidebarPadding + rowPaddingX + rowIcon/2`; an icon button's centre sits at
 * `inset + iconButton/2`. Solving for `inset` gives the expression below —
 * without it the footer icons sat ~6pt left of the row icons.
 */
export function sidebarIconInset(tokens: Tokens): number {
  return (
    tokens.strip.sidebarPadding +
    tokens.strip.rowPaddingX +
    (tokens.strip.rowIcon - tokens.strip.iconButton) / 2
  );
}

export interface GlyphProps {
  glyph: string;
  /** Font size in px. */
  size: number;
  color: string;
  /**
   * Side of the square the glyph is centred in. Becomes `lineHeight`, which is
   * what actually centres it vertically.
   */
  box: number;
}

export function Glyph(props: GlyphProps) {
  return (
    <text
      style={{
        color: props.color,
        fontSize: props.size,
        width: '100%',
        textAlign: 'center',
        lineHeight: props.box,
      }}
    >
      {props.glyph}
    </text>
  );
}

export interface IconButtonProps {
  testId: string;
  glyph: string;
  tokens: Tokens;
  onClick: () => void;
  /** Defaults to `tokens.strip.iconButton`. */
  size?: number;
  /** Defaults to `tokens.font.chip`. */
  fontSize?: number;
  color?: string;
}

export function IconButton(props: IconButtonProps) {
  const { tokens } = props;
  const size = props.size ?? tokens.strip.iconButton;
  return (
    <div
      testId={props.testId}
      onClick={props.onClick}
      style={{
        width: size,
        height: size,
        flexShrink: 0,
        // The glyph centres itself (see the module note); these keep the text
        // element itself filling the button so `width: '100%'` means the button.
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        borderRadius: tokens.radius.tab,
        cursor: 'pointer',
        hover: { backgroundColor: tokens.bg.glassHover },
      }}
    >
      <Glyph
        glyph={props.glyph}
        size={props.fontSize ?? tokens.font.chip}
        color={props.color ?? tokens.fg.muted}
        box={size}
      />
    </div>
  );
}
```

### 9.2 `.claude/skills/fixing-gpuix-layout/SKILL.md`

```markdown
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
```

> Note: the skill’s “macOS window chrome” paragraph still quotes the textbook
> 70pt / `(h-12)/2` figure. The **implemented** numbers in this handover are
> the measured ones (`TRAFFIC_LIGHTS_RIGHT = 78`, `trafficLightY = 13`). Prefer
> the measured numbers when they disagree.

### 9.3 `.claude/skills/fixing-gpuix-layout/scripts/window-shot.sh`

`chmod +x` after writing.

```bash
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
```

Region crop needs Pillow (`PIL`). If `python3` cannot import it, capture the
whole window and `Read` that instead.

---

## 10. testIds (for screenshots and future tests)

| testId | Where |
|---|---|
| `app-root` | unchanged |
| `frame` | unchanged |
| `sidebar` | vertical column |
| `sidebar-header` | |
| `sidebar-toggle` | `≡` (also on horizontal `TitleBar`) |
| `new-tab` | `+` in header (vertical) or strip (horizontal) |
| `sidebar-divider` | 1px |
| `sidebar-footer` | |
| `sidebar-palette` | `⋯` |
| `sidebar-new-session` | `□` |
| `content` | right column |
| `content-header` | |
| `titlebar` | horizontal only |
| `tab-strip` | unchanged |
| `session-chip` | unchanged |
| `tab-${id}` | row (layout only) |
| `tab-${id}-activate` | **new** sibling hit target |
| `tab-${id}-close` | unchanged |
| `banner` / `banner-text` / `banner-action` | unchanged |
| `command-palette` / `palette-input` / `palette-row-*` / `palette-empty` | unchanged |
| `toasts` / `toast-${id}` | unchanged |

Removed: `titlebar-spacer`.

---

## 11. Out of scope (do not implement from this brief)

These also differ from `origin/main` in the source working tree, but they are
**not** chrome:

- `crates/st-native/**` — terminal cell geometry / paint / props
- `vendor/gpuix` — `register_global_factory`, `DispatchMouse` double-lease
  panic, pub exports (`GpuixView`, `apply_interactive_styles`)
- `packages/app/src/app.tsx` preload-native import (packaged binary)
- `scripts/{env,run,package-macos}.sh`, `justfile`, `bunfig.toml`
- `packages/app/src/server/{paths,ensure*}.ts`
- `docs/DEV.md`, `docs/PINS.md`

If the other machine cannot launch the app at all, that is a native/packaging
problem, not this chrome work.
