# 04 — Client native layer: `st-client-core`, `st-native`, and the gpuix patch

> **Addendum (00-grilling §F):** Q43 selection and scroll offset are sent on the **Data Plane**; Q44 only the visible `<terminal-grid>` is mounted, `st-client-core` keeps an LRU of 4 warm Replicas and other Tabs in the active Session use Passive attach; Q40 non-matching Replica is letterboxed until the next Delta; Q48 cursor hidden while scrolled up, Linux primary selection on, `bold_is_bright` config flag default false. §1's native-module resolution was updated to `NAPI_RS_NATIVE_LIBRARY_PATH` (verified in 05 §8).

Scope: everything between the data-plane socket and the pixels. Covers the two Rust crates that make up the native side of the client, the `<terminal-grid>` custom element, and the one-hook patch to gpuix. Decisions from `00-grilling.md` (Q11–Q16, Q23–Q28, Q36) are taken as given; anything they do not settle is listed under *Open questions* at the end. Plan only — signatures and structs below are illustrative.

Grounding: gpuix 0.6.0 (`packages/native` = crate `gpuix-native`, napi-rs v3, `crate-type = ["cdylib","rlib"]`, GPUI as path dep `../../zed/crates/gpui`), its `CustomElement` / `CustomElementFactory` / `CustomElementRegistry` traits, and Zed's `crates/terminal_view/src/terminal_element.rs` as the reference for painting an alacritty-style grid in GPUI.

---

## 1. gpuix integration strategy (Q12)

### 1.1 Vendoring

`vendor/gpuix` is a git submodule pointing at our fork branch `superterminal` (fork of `remorses/gpuix`, based on the `v0.6.0` tag). gpuix itself vendors Zed as a submodule (`zed/`), so we initialise recursively and **pin exactly the Zed commit gpuix pins** (Q36a) — we never bump Zed independently. The Rust workspace at the repo root lists `vendor/gpuix/packages/native` as a path member so `st-native` can depend on `gpuix-native = { path = "../../vendor/gpuix/packages/native" }`. `@gpuix/react` and `@gpuix/core` are consumed from npm at the matching version; only the native crate comes from the submodule.

### 1.2 The hook — two candidates

**(a) `GpuixRenderer::init_with_factories(options, factories)`** — a new Rust-only associated function (not `#[napi]`) that does what `init` does but calls `registry.register(f)` for every factory before the first render. `init` becomes `init_with_factories(options, vec![])`.

**(b) `pub fn register_global_factory(Box<dyn CustomElementFactory>)`** — pushes into a process-global `Mutex<Vec<Box<dyn CustomElementFactory + Send>>>` that `GpuixRenderer::init` drains into the registry after `with_defaults()`.

**Recommendation: (b).** Reason: the napi class `GpuixRenderer` is *constructed from JS* (`new GpuixRenderer(callback)` inside `@gpuix/react`'s renderer bootstrap), so a downstream Rust crate never holds the `GpuixRenderer` value and cannot call an alternative `init` on it without also re-implementing the JS side. A global that is consulted *inside* the existing `init` needs zero changes to `@gpuix/react` or `@gpuix/core`. Consequence: factories must be `Send` to sit in a global before the GPUI thread exists — `TerminalGridFactory` holds only an `Arc<DataPlaneHandle>`, so that is free. The patch is ~30 lines in `renderer.rs` + `custom_elements/mod.rs`: the global, the `register_global_factory` fn, one `for f in drain() { registry.register(f) }` loop in `init`, and a `pub use` of the trait types. Factory registration must happen before `init`; we guarantee this by doing it in the `#[napi(module_init)]`-style entry of our own cdylib (see 1.3), which runs when `require()` loads the module — strictly before JS can call `init`.

### 1.3 Who owns the cdylib — evaluate both, pick one

**Option A: separate `st-native` cdylib alongside the stock `@gpuix/native` `.node`, depending on `gpuix-native` as rlib only for types.** Rejected outright: `@gpuix/react` would `require('@gpuix/native')` and load the original gpuix `.node` next to ours — **two GPUI runtimes in one process**, two registries, and our factory in the wrong one.

**Option B: one cdylib, `st-native`, with the patched `gpuix-native` compiled in.** `st-native` depends on `gpuix-native` as **rlib** and `pub use gpuix_native::*;`. napi-rs v3 registers `#[napi]` items through `#[ctor]`-style statics emitted in the crate they are compiled in; when that crate is an rlib linked into our cdylib the statics are retained only if the linker keeps them (Linux does by default; macOS `-dead_strip` may not). We therefore reference them explicitly (`#[used] static KEEP = gpuix_native::__napi_keepalive;`) behind a `napi-exports` feature added by the patch. If M0 shows this is flaky on macOS, the fallback stays inside Option B: gate gpuix's `#[napi] struct GpuixRenderer` (the ~200-line wrapper, not the engine) behind the feature and define a thin delegating `#[napi] struct GpuixRenderer` in `st-native` with the identical JS shape.

**Pick: Option B.** One `.node`, one GPUI, one registry. Output: `superterminal-native.<platform>.node` exporting the `GpuixRenderer` class `@gpuix/react` expects (`init, applyBatch, tick, focusElement, scrollTo, getWindowSize, …`) plus our additions (`stConnectDataPlane`, `stReadProp` — §3).

**Consequence for `require('@gpuix/native')` resolution.** `@gpuix/react` hard-codes the specifier. Decision (verified in `05-client-app.md` §8 against gpuix 0.6.0's `packages/native/index.js`): the napi-rs loader checks `process.env.NAPI_RS_NATIVE_LIBRARY_PATH` first and `require`s that file directly, so the app sets that variable to our `superterminal-native.<triple>.node` in a `bunfig.toml` `[run]`/`[test]` preload before `@gpuix/react` is evaluated. Our `.node` is a drop-in superset of gpuix's (re-exports `GpuixRenderer`/`TestGpuixRenderer` and registers `TerminalGridFactory`). Rejected: root `package.json` `overrides` with a `workspace:` value (Bun 1.4.0 documents only `npm:`/`catalog:` values), `bunfig [install.overrides]` (undocumented in 1.4.0), `bun build --define` (rewrites identifiers, not specifiers), and publishing our package *as* `@gpuix/native` (confusing).

### 1.4 Upstreaming

PR to `remorses/gpuix`: "Allow downstream crates to register custom element factories before `init`" — adds `register_global_factory`, the `napi-exports` feature, and `pub` visibility on the trait module. Small, opt-in, no JS change. If merged: our submodule moves to upstream `main`+tag and the fork branch is deleted. If rejected (Q36b): we keep the ~30-line patch and rebase it on each gpuix bump; ADR-0006 records this.

---

## 2. Build pipeline

- `crates/st-native/package.json` (inside `packages/native` for the JS side; the crate lives at `crates/st-native`) declares `"napi": { "binaryName": "superterminal-native", "targets": ["aarch64-apple-darwin","x86_64-unknown-linux-gnu","x86_64-pc-windows-msvc"] }`.
- `just native` = `cd crates/st-native && napi build --platform --release --output-dir ../../packages/native` → `packages/native/superterminal-native.linux-x64-gnu.node` (etc.). `just native-debug` drops `--release` (debug GPUI is usable but ~5× slower to paint; fine for logic work).
- **First build time**: GPUI pulls ~600 crates (blade/wgpu, cosmic-text/font-kit, taffy, calloop/wayland on Linux, objc/metal on macOS). Expect **10–20 min cold**, 3–6 min with warm `sccache` (`RUSTC_WRAPPER` set in the `justfile`; CI caches `~/.cargo/registry` and `target/` keyed on `Cargo.lock` + Zed commit). Incremental rebuild after a change in our crate: 20–60 s, link-dominated — `mold`/`lld` via `.cargo/config.toml`.
- **Profile**: `[profile.release] debug = 1, lto = "thin", codegen-units = 16`; `panic = "unwind"` so napi can catch panics at the boundary.
- **`bun --hot` and a rebuilt `.node`**: hot reload re-evaluates JS modules, but a Node-API addon is `dlopen`ed once per process and cannot be unloaded; a second `require` returns the cached handle, and overwriting a mapped `.node` is undefined behaviour. So `just dev` runs `bun --hot packages/app/src/main.tsx` under `watchexec -w packages/native/*.node -r`, which **restarts the Bun process** when the `.node` changes; TSX edits alone stay hot. Because the server owns all state, a client restart loses only window geometry (Q17) — it is the "half a dock bounce" reconnect, exercised on every native rebuild.

---

## 3. `<terminal-grid>` custom element

Type string: `"terminal-grid"`. React usage: `<terminal-grid surfaceId={id} fontFamily=… theme={palette} passthroughKeys={[...]} onTitle={…} />` — `@gpuix/react` sends non-style props via `setCustomProp(id, key, json)` and wires `on*` handlers to events emitted via `emit_event_full(callback, id, name, payload)`.

**Props (set_prop)**

| key | type | notes |
|---|---|---|
| `surfaceId` | `u64` | changing it detaches the old surface and attaches the new one; the element keeps no data of its own |
| `fontFamily`, `fontSize`, `lineHeight` | string, f32 px, f32 multiplier | any change → recompute cell metrics, clear `RunCache`, send `Resize` |
| `theme` | JSON `{ansi:[16 hex], fg, bg, cursor, cursorText, selectionBg, selectionFg?}` | §10 |
| `cursorStyle` | `"block"\|"beam"\|"underline"`, `cursorBlink: bool` | default when the program has not set DECSCUSR |
| `padding` | `{top,right,bottom,left}` px | inside the element bounds; cells are laid out in the remainder |
| `passthroughKeys` | `[string]` keystroke names | §7 — keys the element must *not* consume |
| `scrollbar` | `"auto"\|"always"\|"never"` | |
| `focused` | bool (optional) | React can request focus; the element also calls `window.focus()` on click |

**Events (emit to React)**: `focus`, `blur`; `title {title}`; `exited {code}`; `bell`; `selection {hasSelection, text?: null}` (text is *not* included — copy is explicit, §9); `scroll {offset, historyLen, rows}` for a React-side scrollbar/indicator to stay in sync (throttled to once per frame); `resize {cols, rows}`; `modes {altScreen, mouse, bracketedPaste}` (so React can e.g. change the paste shortcut label).

**Imperative reads (get_prop)**: `scrollOffset` → number; `contentLines` → total lines (history + rows); `selectionText` → string (used by the Copy command); `cellSize` → `{w,h}`; `size` → `{cols,rows}`. gpuix does not currently expose `get_prop` over napi ("phase 2"); the patch adds `#[napi] fn get_custom_prop(&self, id: u32, key: String) -> Option<serde_json::Value>` alongside the factory hook — it is the same PR. Imperative *writes* that are not state (`copy`, `scrollToBottom`, `clearScrollback`) are modelled as set_prop on a monotonically increasing `command: {seq, name, args}` prop, the standard trick for one-shot commands in a retained tree.

---

## 4. Replica

```rust
pub struct Replica {
    pub cols: u16, pub rows: u16,
    pub visible: Vec<Row>,             // rows.len() == rows
    pub history: RingBuffer<Row>,      // oldest → newest; capacity = config.scrollback (default 10_000)
    pub history_base: u64,             // absolute index of history[0] in the server's scrollback
    pub history_len_server: u64,       // what the server says it has (for the scrollbar)
    pub styles: StyleTable,            // Vec<Style>; index = style_idx from the wire
    pub cursor: CursorState,           // { col, row, shape, visible, blink }
    pub modes: Modes,                  // alt_screen, mouse: MouseMode, bracketed_paste, app_cursor_keys, focus_events
    pub title: String,
    pub seq: u64,                      // last applied Delta seq
    pub exited: Option<i32>,
}
pub struct Row { pub cells: Vec<PackedCell>, pub extra_graphemes: Vec<SmolStr>, pub dirty: bool, pub wrapped: bool }
#[repr(C)] #[derive(Clone, Copy)] pub struct PackedCell { pub cp: u32, pub style: u16, pub flags: u8, pub _pad: u8 } // 8 B, Q16
pub struct Style { pub fg: Color, pub bg: Color, pub attrs: Attrs /* bold,dim,italic,underline(kind),strike,inverse,hidden,blink */ }
```

`flags` bits: `WIDE`, `WIDE_SPACER`, `HAS_EXTRA` (grapheme cluster — the cluster's full text lives in `extra_graphemes[cp as index]`, i.e. `cp` is reinterpreted as an index into that row's side table), `LEADING_SPACER` (wide char wrapped from previous line). `Color` is `enum { Default, Indexed(u8), Rgb(u8,u8,u8) }` packed into a `u32` on the wire; resolution against the palette happens at paint time so a theme change does not touch the replica.

**Delta application** (`Replica::apply(&mut self, d: &Delta) -> Result<(), Gap>`):
1. `d.seq != self.seq + 1` (including going backwards after a server restart) → return `Gap { have, got }`; the delta is dropped, not buffered — the caller requests a Snapshot, which supersedes everything.
2. `styles.extend(d.new_styles)` (server only appends; indices are stable per attach).
3. If `d.scrollback_appended = n > 0`: move the top `n` rows of `visible` *as they were before this delta* into `history` (`push_back`, evicting the oldest and bumping `history_base` when full); `history_len_server += n`. Rows scroll off *before* dirty rows overwrite them.
4. For each `DirtyRow { y, cells, extra }` → replace `visible[y]`, mark `dirty`.
5. Copy `cursor`, `modes`, `title`, `exited`; `seq = d.seq`. If `d.resize` is present, resize `visible` (the server marks every row dirty after a resize).

**Snapshot application** replaces `visible`, `styles`, `cursor`, `modes`, `seq`, clears `history` and sets `history_base = history_len_server`; history is re-fetched lazily as the user scrolls. Deltas for surfaces we are not attached to are ignored.

**History paging (Q25)**: when the viewport needs absolute row `r < history_base`, `fetch_history(surface, from = r.saturating_sub(1000), count = 1000)`; `HistoryRows{from, rows}` is `push_front`ed. One outstanding fetch per surface; blank rows are painted meanwhile.

**Memory budget**: 10 000 history rows × 200 cols × 8 B = 16 MB per surface worst case. Rows entering history are truncated at the last non-default cell and re-padded on paint, so a typical 80-col footprint is 2–4 MB; `extra_graphemes` is an empty `Vec` (24 B, no heap) for almost every row. `scrollback` comes from config (hard max 100 000); the ring evicts oldest. When a `<terminal-grid>` unmounts (tab hidden) the replica is kept but `history.shrink_to(1000)` — the rest is refetchable.

---

## 5. Data-plane client

A **dedicated OS thread** ("st-dataplane") runs a `tokio` current-thread runtime that owns the single Unix socket (`$XDG_RUNTIME_DIR/superterminal/data.sock`), the framing codec (`u32 len | u16 type | postcard payload`, Q15), the `Hello` handshake, and reconnect with backoff. It is started once from the napi `stConnectDataPlane(path)` call (or lazily by the first factory `create`).

```rust
#[derive(Clone)] pub struct DataPlane { tx: mpsc::UnboundedSender<Outbound>, state: Arc<Shared> }
impl DataPlane {
    pub fn attach(&self, s: SurfaceId, listener: Arc<dyn Wake>);
    pub fn detach(&self, s: SurfaceId);
    pub fn send_input(&self, s: SurfaceId, bytes: Vec<u8>);
    pub fn resize(&self, s: SurfaceId, cols: u16, rows: u16);
    pub fn fetch_history(&self, s: SurfaceId, from: u64, count: u32);
    pub fn set_selection(&self, s: SurfaceId, sel: Option<Selection>);   // Q24 persistence
    pub fn replica(&self, s: SurfaceId) -> Option<MappedMutexGuard<'_, Replica>>;
}
struct Shared { replicas: parking_lot::Mutex<HashMap<SurfaceId, ReplicaSlot>>, connected: AtomicBool }
struct ReplicaSlot { replica: Replica, wake: Arc<dyn Wake>, pending_paint: AtomicBool }
```

Inbound `Delta`/`Snapshot`/`HistoryRows` are applied **on the data-plane thread** directly into `replicas` under the mutex; application takes microseconds and the only other locker is paint, which holds the lock just long enough to copy the visible rows (§6). A per-surface channel drained by GPUI was rejected: it moves the applying onto the frame budget for no benefit. On `Gap` the thread sends `RequestSnapshot` itself.

**Waking GPUI.** Two candidates:
- gpuix `tick()`: on macOS JS calls `renderer.tick()` in a loop and it pumps `platform.pump_events()`; on Linux/Windows GPUI runs its own event loop on a spawned thread and `tick()` returns `true` without doing anything. Not portable, and nothing in it notices a replica change.
- GPUI-native: at `create()` the element captures `cx.to_async()` (`AsyncApp`) and a `WeakEntity<GpuixView>`; `Wake` does `async_app.update(|cx| weak.update(cx, |_, cx| cx.notify()))`, which schedules onto GPUI's foreground dispatcher and pings the platform run loop (`CFRunLoop` source on macOS, `calloop` ping on Linux) from any thread.

**Pick: GPUI-native wake via `AsyncApp` + `cx.notify()`** — the only path identical on both platforms, and what Zed does for PTY output.

**≤1 repaint per frame (Q27):** `ReplicaSlot.pending_paint: AtomicBool`. On each applied delta: `if !pending_paint.swap(true) { wake() }`, so N deltas between frames cost one `notify`. `paint()` stores `false` *after* copying rows, so a delta landing mid-paint schedules the next frame instead of being lost. The server's ~120 Hz throttle bounds wake rate; GPUI coalesces notifies into one frame.

---

## 6. Painting

`TerminalGridElement` is a `CustomElement` whose `render()` returns a `gpui::canvas(prepaint, paint)`-style element wrapped in a `div()` carrying `Interactivity` (for focus/mouse/key) and a `FocusHandle`. Following Zed's `TerminalElement`, all per-frame work is split into *layout* (compute quads and runs from the replica) and *paint* (issue GPUI draw calls).

**Cell metrics** (once per font change, cached in the element): `cell_w = text_system.advance(font_id, font_px, 'M')` (Zed uses `'m'`; either is fine for a monospace font — we use `'M'` per Q26), `line_h = font_px * line_height` (default 1.2). `cols = floor((w - pad.l - pad.r) / cell_w)`, `rows = floor((h - pad.t - pad.b) / line_h)`; if they differ from the replica's → `dataplane.resize()`, and we keep painting the old grid until the server's resize delta arrives (no local reflow).

**Per frame**:
1. Lock the replica, copy the rows visible at the current scroll offset (history slice + visible slice) into a frame-local `Vec<RowView>` with cursor/modes/selection; unlock. Lock hold time is one memcpy ≤ rows×cols×8 B (≈ 160 KB at 200×100).
2. **Background quads**: per row, resolve each cell's `bg` (after inverse swap and selection), merge adjacent equal-bg cells into `LayoutRect { x, y, w_cells, color }`, skip cells whose bg is the element default (the `div` paints it). `window.paint_quad(fill(bounds, color))`.
3. **Glyph runs**: group adjacent cells with identical `StyleKey { fg, bold, italic, dim, underline, strike }` into `BatchedRun { col, text, cells, key }` (Zed's `BatchedTextRun`); all-blank runs are dropped. Look up `RunCache: LruCache<(u64 /*row content hash*/, StyleKey), ShapedLine>` (capacity 2× rows); miss → `window.text_system().shape_line(text, font_px, &[TextRun{len, font, color, ..}], None)`. Cleared on font/size change; a palette swap costs one all-miss frame because color is part of the key. Paint via `shaped.paint(origin, line_h, window, cx)`. This cache is measured in M2 before anything else is built (Q36c).
4. **Decorations**: underline (single/double; curly drawn as dashed in v1) and strikethrough as thin quads at font-metric offsets, after glyphs.
5. **Selection overlay**: `HighlightedRange`-style quads per line span in `selectionBg`; selected cells get their own `StyleKey` so glyphs render in `selectionFg` when configured.
6. **Cursor**: painted only if `cursor.visible && scroll_offset == 0 && (focused ? blink_on : true)`. Block: filled quad in `cursor` colour with the covered glyph re-painted in `cursorText`; beam: 2 px quad at cell left; underline: 2 px quad at cell bottom; unfocused → 1 px hollow block. **Blink**: a `cx.spawn` 530 ms timer exists only while focused and blink is enabled (prop or DECSCUSR); key input resets the phase to "on"; dropped on blur so a background terminal costs zero frames.
7. **Wide chars**: a `WIDE` cell counts 2 in the run's `cells`, the following `WIDE_SPACER` is skipped, bg quads cover both. `HAS_EXTRA` cells splice their grapheme string in. Emoji use GPUI's font fallback; overshoot beyond `cells × cell_w` is clipped with `window.with_content_mask`.
8. **Attribute mapping**: bold + indexed fg 0–7 → 8–15 when `theme.boldIsBright`; dim → fg alpha × 0.7 (Zed's constant); inverse → swap fg/bg *before* selection; hidden → no glyphs; blink attribute ignored in v1.
9. **Text colour caveat**: GPUI text colour does **not** inherit from parents (gpuix README warning) — every `TextRun` carries an explicit `color`; we never rely on a `div().text_color()` ancestor.

**Scrollbar**: vertical track in the right padding; thumb height `rows / contentLines × track_h` (min 24 px), position from `scroll_offset`; two quads, drawn only when `contentLines > rows` and (`scrollbar == always` or the mouse moved within 1.5 s). We paint it ourselves rather than via gpuix's `ScrollHandle` because the content is virtual (interaction in §8).

---

## 7. Keyboard input

The element's `div` holds a `FocusHandle` (`cx.focus_handle()` at `create`), `track_focus(&handle)`, and `on_key_down`. Focus is taken on mouse-down and when React sets `focused`. `focus`/`blur` events are emitted from `on_focus_in/out`.

**Encoding** lives in `st-client-core::keys` — pure functions, unit-tested:

```rust
pub struct KeyInput<'a> { pub key: &'a str /* gpui keystroke key, e.g. "a", "enter", "f5", "up" */, pub mods: Mods, pub text: Option<&'a str> /* IME-composed / shifted char */ }
pub fn encode_key(k: &KeyInput, modes: &KeyModes) -> Option<Vec<u8>>;   // None = not a terminal key → let GPUI propagate
```

Table (xterm-compatible): printable → `text` bytes (UTF-8); `Alt`+printable → `ESC` + bytes (macOS Option-as-Alt is a config flag, default off); `Ctrl`+letter → `0x01..0x1A`, plus `Ctrl+[ \ ] ^ _`, `Ctrl+Space` → NUL; `Enter` → `\r`, `Tab` → `\t`, `Shift+Tab` → `ESC[Z`, `Backspace` → DEL (config: BS), `Escape` → `ESC`; arrows → `ESC[A..D`, or `ESC O A..D` under DECCKM (application cursor keys), modified → `ESC[1;{m}A` with `m = 1 + shift·1 + alt·2 + ctrl·4`; Home/End → `ESC[H/F` (`ESC O H/F` in app mode); Insert/Delete/PgUp/PgDn → `ESC[2~ 3~ 5~ 6~` (+`;m`); F1–F4 → `ESC O P..S`, F5–F12 → `ESC[15~ 17~ 18~ 19~ 20~ 21~ 23~ 24~`. Application keypad and the Kitty keyboard protocol are deferred (hook: `KeyModes.kitty_flags`).

**Passthrough allowlist.** `passthroughKeys` is a list of gpuix keystroke strings (`"cmd-t"`, `"cmd-k"`, `"cmd-w"`, `"cmd-shift-]"`, `"ctrl-shift-t"` …). `on_key_down` checks it first; a match **returns without `cx.stop_propagation()`**, so GPUI bubbles the event to the app-root `<div onKeyDown>` in React (Q23). Everything else the encoder accepts is sent and stopped. React derives the list from the command registry (the single source of truth), so the element never consumes a chord that is a command. Platform defaults: macOS — every `cmd-*` chord; Linux — `ctrl-shift-*` (the terminal convention for chrome shortcuts), `alt-<digit>`, `super-*`.

**IME**: implement GPUI's `InputHandler` on the element — `replace_text_in_range` sends committed text, `replace_and_mark_text_in_range` stores marked text painted as an overlay at the cursor, `bounds_for_range` returns the cursor cell rect so the candidate window docks correctly. Dead keys take the same path. CJK edge cases and marked-text width deferred to M4.

---

## 8. Mouse

Hit-testing: `cell_at(pos) -> Option<(col, row_abs)>` where `row_abs = scroll_top + (y - pad.t) / line_h` and `col = (x - pad.l) / cell_w`, clamped. Clicks in the right padding hit the scrollbar first.

**Selection** (`st-client-core::selection`): `Selection { anchor: Point{abs_row, col}, head, kind: Simple|Word|Line|Block }`, normalised on read. Click sets the anchor and clears; drag moves the head, snapping to cell edges at the x-midpoint; double-click → `Word` (alnum + config `wordChars`, default `_-./~`), triple-click → `Line` (spanning `wrapped` rows); `Alt`+drag → `Block`. Wide cells select as a unit. Dragging past the top/bottom edge auto-scrolls. On mouse-up: `dataplane.set_selection()` for persistence (Q24) and emit `selection`.

**Mouse mode override**: `modes.mouse != None` and Shift *not* held → report to the program; otherwise → local selection. Reporting (`st-client-core::mouse`):

```rust
pub enum MouseProto { X10, Normal, ButtonEvent, AnyEvent }   // modes 9, 1000, 1002, 1003
pub enum MouseEnc { Default, Utf8, Sgr }                     // 1005, 1006 (SGR is what we expect modern programs to set)
pub fn encode_mouse(ev: &MouseEv, proto: MouseProto, enc: MouseEnc) -> Option<Vec<u8>>;
```

Default: `ESC[M Cb Cx Cy` with `Cb = 32 + button + mods + (motion?32:0)`, coordinates `32 + n` capped at 223; SGR: `ESC[<b;x;yM` / `m` on release with no cap. Motion events only in `ButtonEvent` (while pressed) / `AnyEvent`; we throttle motion to one report per cell change.

**Wheel**: not alt-screen → scroll the viewport by `lines = delta_y / line_h` (pixel-precise for trackpads, 3 lines per notch for wheels), clamped to `[0, history_len]`; alt-screen with mouse mode → wheel reported as buttons 64/65; alt-screen *without* mouse mode → **arrow-key emulation**: `n` × `ESC[A`/`ESC[B` (`ESC O A/B` in app-cursor mode), which makes `less`, `vim`, `man` scroll as users expect (config `altScreenScroll: arrows|off`).

**Scrollbar interaction**: mouse-down on thumb starts a drag mapping pixel delta to rows; mouse-down on track pages up/down; hover expands the thumb from 6 px to 10 px.

**Auto-scroll rule (Q25)**: `scroll_offset` is stored as *distance from bottom* (0 = following). Output while `offset == 0` stays at the bottom; while `offset > 0` new output does not move the view (history grows underneath, so `offset` in "from bottom" terms increases by `scrollback_appended`, and the same rows stay on screen). Any key that produces input → `offset = 0` (jump to bottom), as in every mainstream terminal. `Cmd/Ctrl+End` → 0 explicitly.

---

## 9. Clipboard

- **Copy** happens only on the explicit React `Copy` command, delivered as `command: {name:"copy"}`; the element writes `cx.write_to_clipboard(ClipboardItem::new_string(text))` itself so the text never crosses napi. No copy-on-select in v1 (Linux primary selection: open question). Text extraction trims trailing whitespace per row, joins rows with `\n` except across `wrapped` rows, drops wide spacers, expands `extra_graphemes`.
- **Paste**: `cx.read_from_clipboard()` → `st-client-core::paste::prepare(text, modes) -> Vec<u8>`: normalise `\r\n`/`\n` → `\r` (programs expect CR for Enter) unless bracketed paste is on, keep a trailing newline (xterm behaviour; config to strip), wrap in `ESC[200~ … ESC[201~` when `modes.bracketed_paste`, and strip any embedded `ESC[201~` (paste-injection guard). Multi-line paste without bracketed mode → React confirmation (config `confirmMultilinePaste`, default on).
- **Chunking**: `Input` frames are capped at 64 KiB; the data-plane thread splits large pastes into consecutive frames. The server writes them in order with its own PTY backpressure, so no client-side pacing is needed.

---

## 10. Theme and palette

`Palette { ansi: [Rgba; 16], fg, bg, cursor, cursor_text, selection_bg, selection_fg: Option<Rgba>, bold_is_bright: bool }` built from `config.toml` `[theme]` by React and passed as the `theme` prop (so the config parser lives in one place, TS). Defaults: a neutral dark palette (xterm-ish colours, `bg #1e1e1e`, `fg #d4d4d4`).

Resolution (`st-client-core::color::resolve(Color, &Palette) -> Rgba`): `Default` → fg/bg; `Indexed(0..16)` → `ansi[i]`; `Indexed(16..232)` → 6×6×6 cube: `i-16 = 36r+6g+b`, component `c → if c==0 {0} else {55+40c}`; `Indexed(232..256)` → grey `8 + 10*(i-232)`; `Rgb` passthrough. Cube and greys are computed once into a `[Rgba; 256]` table when the palette changes. The element background is `bg` with the window's alpha when `windowBackground` is blurred/transparent (Q28) — cells whose bg is `Default` are not painted (they show the blur), cells with explicit bg are opaque.

Hook for later: OSC 4/10/11 (program-set palette) arrive in the server's mode/state and could override entries per surface; not in v1.

---

## 11. Module layout

### `crates/st-client-core` — pure Rust, **no GPUI, no napi**; depends on `st-proto`, `postcard`, `tokio` (net + rt), `parking_lot`, `smol_str`.

| file | purpose |
|---|---|
| `src/lib.rs` | re-exports; `SurfaceId` alias |
| `src/replica/mod.rs` | `Replica`, `apply_delta`, `apply_snapshot`, `Gap` |
| `src/replica/row.rs` | `Row`, `PackedCell`, flags, trimming/padding helpers |
| `src/replica/ring.rs` | `RingBuffer<Row>` with `push_front/back`, `shrink_to`, absolute indexing |
| `src/replica/style.rs` | `StyleTable`, `Style`, `Attrs`, `Color` (wire ↔ enum) |
| `src/replica/view.rs` | viewport math: `scroll_offset` (from bottom) → absolute row range, `RowView` snapshot for the painter |
| `src/selection.rs` | `Selection`, word/line/block extension, `text_of(&Replica)` |
| `src/keys.rs` | `encode_key`, `KeyModes`, xterm tables |
| `src/mouse.rs` | `encode_mouse`, wheel → arrows emulation, protocol/encoding enums |
| `src/paste.rs` | `prepare` (normalisation, bracketing, injection guard) |
| `src/color.rs` | `Palette`, 256-table computation, `resolve`, bold-bright/dim rules |
| `src/dataplane/mod.rs` | `DataPlane` handle, `Shared`, `Wake` trait, `ReplicaSlot` |
| `src/dataplane/conn.rs` | tokio task: connect/handshake/reconnect, framing, dispatch |
| `src/dataplane/codec.rs` | `u32 len | u16 type | postcard` encode/decode (round-trip tests) |
| `tests/replica_props.rs` | proptest: random delta streams vs. a model grid; gap → snapshot converges |
| `tests/keys.rs`, `tests/mouse.rs` | golden byte sequences |
| `tests/fixtures/*.snap` | vt-conformance fixtures shared with the server tests (Q33) |

Everything here runs under `cargo test` on a headless CI box. `Wake` is a trait so tests use a counting fake.

### `crates/st-native` — cdylib; depends on `gpuix-native` (rlib, patched), `gpui`, `napi`, `st-client-core`.

| file | purpose |
|---|---|
| `src/lib.rs` | `pub use gpuix_native::*`; napi module init: `register_global_factory(Box::new(TerminalGridFactory::new(dp)))`; `#[napi] fn st_connect_data_plane(path)`, `st_read_prop` |
| `src/factory.rs` | `TerminalGridFactory: CustomElementFactory` (`element_type() == "terminal-grid"`, `create()` → captures `AsyncApp` + `WeakEntity<GpuixView>` for `Wake`) |
| `src/element.rs` | `TerminalGridElement: CustomElement` — props, `render()` building the `div`+canvas, event emission, command prop dispatch |
| `src/layout.rs` | frame layout: metrics, `LayoutRect`, `BatchedRun`, merge/group passes, `RunCache` |
| `src/paint.rs` | paint order §6: quads, runs, decorations, selection, cursor, scrollbar |
| `src/cursor.rs` | cursor shapes, blink timer task |
| `src/input.rs` | `on_key_down` glue: gpui `Keystroke` → `KeyInput`, passthrough check, `InputHandler` (IME) impl |
| `src/mouse.rs` | mouse listeners → selection / reporting / wheel / scrollbar drag |
| `src/clipboard.rs` | copy/paste glue to `cx.write_to_clipboard` / `read_from_clipboard` |
| `src/wake.rs` | `GpuiWake: Wake` using `AsyncApp::update` + `cx.notify()` |
| `src/theme.rs` | JSON `theme` prop → `Palette`; gpui `Hsla` conversion |
| `build.rs` | `napi_build::setup()` |

Tests: `st-native` has only a smoke test that the factory registers and `render()` produces an element under GPUI's `TestAppContext` (Zed's test harness, headless); everything with logic is in `st-client-core`. The perf harness (Q33 item 5) lives in `packages/app` and reads frame times the element logs via a `stats` get_prop.

### `vendor/gpuix` patch (fork branch `superterminal`)
`packages/native/src/custom_elements/mod.rs`: `pub` the traits and registry; `register_global_factory`; `drain_global_factories`. `packages/native/src/renderer.rs`: drain into registry after `with_defaults()`; add `get_custom_prop` napi method; `napi-exports` feature gate. `Cargo.toml`: the feature. `packages/react`: no change.

---

## 12. Open questions

1. **napi re-export mechanics (§1.3)**: confirm on day 1 of M0 whether `#[napi]` registration symbols in the rlib survive macOS `-dead_strip`; spike the thin delegating `GpuixRenderer` fallback in the same session so the choice is empirical.
2. **`AsyncApp` thread-safety**: `AsyncApp` is `!Send` in recent Zed; the cross-thread wake may need `BackgroundExecutor::spawn` + a foreground `Task`, or a run-loop ping exposed by the gpuix patch. Verify against the pinned Zed commit; the `Wake` trait isolates the answer.
3. **Cursor while scrolled up**: hidden (chosen above) vs. drawn at its absolute position when still on screen (iTerm2).
4. **Linux primary selection**: copy-on-select is out, but X11/Wayland users expect middle-click paste of the *primary* selection (GPUI has `write_to_primary`). Enable by default on Linux?
5. **`boldIsBright`**: platform-dependent default, and theme property vs. config flag.
6. **History page wire format**: 1 000 × 200-col rows = 1.6 MB per page; does `02-protocol.md`'s `Row` allow trailing-blank truncation?
7. **Selection across resize**: the server persists `Selection` in absolute rows (Q17) but columns need not survive a resize. Proposal: server clears selection on resize; confirm in `03-server.md`.
8. **`get_prop` over napi** ships in our patch; if upstream's "phase 2" plugin API lands with a different shape, track it or hold until the next gpuix bump?
9. **Config key names** for Option-as-Alt (macOS), Backspace DEL/BS, `altScreenScroll`, `wordChars` — belong in the config doc.
10. **Frame-time instrumentation**: `stats` get_prop vs. a `tracing` subscriber to file; decide before M2 so shaping-cache numbers are comparable across runs.
