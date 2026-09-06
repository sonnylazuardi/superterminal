# Handover: pointer input (mouse reports, file drop, tab reordering)

Implementation notes for the pointer work that followed the rendering pass in
[`handover-terminal-style.md`](./handover-terminal-style.md): a mouse
coordinate bug that made every TUI click land on the wrong cell, file
drag-and-drop from Finder / Explorer, dragging tabs to reorder them, and
making chrome text stop behaving like prose.

§1–2 are `crates/st-native` and touch neither the protocol nor the server.
§3–4 are the React chrome and touch neither the native crate nor the wire
format.

---

## 0. Goal

| | |
|---|---|
| Goal | The pointer interactions an iTerm user reaches for first — click a TUI's tab, drop a file into the prompt, drag a tab to a new position — behave the way they expect |
| Reference behaviour | iTerm2 3.x for the terminal ones; a browser tab strip for reordering (drag past the neighbour's midpoint, dragging also selects) |

**Done when:**

- In OpenCode (Bubble Tea) running in a tab, clicking a session tab in its
  header switches to it. Before, nothing happened.
- Dragging a PNG from Finder onto the grid types
  `/Users/you/My\ Shot.png ` into the shell; in OpenCode it becomes an
  `[Image 1]` attachment chip. On Windows the same drop types
  `"C:\Users\you\My Shot.png" `.
- Dragging a tab in the sidebar moves it, and the same works in the
  horizontal strip. A plain click still just switches tab.
- Pressing a tab and dragging it does not paint its title blue.
- `cd crates/st-native && cargo +1.97.1 test --lib` → 171 pass, including
  `element::tests::a_click_on_the_top_left_cell_reaches_the_wire_as_1_1`
  and the seven `dnd::tests`.
- `bun test` → 287 pass, including the 18 in `state/tab-drag.test.ts` and
  the two `tabDrag` reducer tests.

---

## 1. Mouse reports were one cell right and one cell down

### Symptom

OpenCode's tab bar ignored clicks. Wheel, drag and motion were also off, but
a click on a one-row-tall target is where it shows.

### Cause — a doubled 1-based offset

Two functions each believed they owned the conversion to the wire's 1-based
coordinates:

| | Where | What it did |
|---|---|---|
| `GridState::report_cell` | `crates/st-native/src/element.rs` | `hit_test` → `(col + 1, row + 1)` |
| `encode_mouse` | `crates/st-client-core/src/mouse.rs` | `x = col + 1; y = row + 1` |

`MouseEvent::cell` is documented **0-based** — "the encoders add the 1-based
offset the wire format wants" — and `st-client-core`'s tests hold that
contract (`(10, 4)` → `ESC[<0;11;5M`). The element was the side in the wrong.
Every report went out at `(col + 2, row + 2)`: a click on screen row 2
arrived as row 3, the blank line under the tab bar.

A unit test had **pinned the wrong answer**: `report_cells_are_one_based`
asserted `(1, 1)` for the top-left cell. That is why it survived — the test
suite was green.

### Fix

`report_cell` returns `(point.col, row)`. Nothing else consumed its value;
selection goes through `cell_at` directly.

Two tests replace the old one:

- `report_cells_are_zero_based_because_the_encoder_adds_the_one` — the unit.
- `a_click_on_the_top_left_cell_reaches_the_wire_as_1_1` — **end to end**
  through the same two functions the press handler calls, asserting the
  bytes: `ESC[<0;1;1M` for the top-left cell, `ESC[<0;3;2M` for the third
  cell of the second row. This is the assertion that would have caught it.

### Why it was hard to see

The pipeline was otherwise complete and correct: modes 1000/1002/1003/1006
flow from `alacritty_terminal` through `Modes` to the client; `reports_to_
program` honours all three tracking modes; SGR and legacy encodings are both
right; press, release, motion and wheel are all sent; Shift hands the mouse
back to the user. With everything present, "clicks do nothing" reads as a
missing feature. It was a `+ 1`.

**When a TUI ignores the mouse, check the coordinate math before the
protocol.** `st probe --dump` will not show client→server input; write the
bytes out in a unit test instead, as the new test does.

---

## 2. Dropping files types their paths

### What iTerm does

A file dragged from Finder onto a terminal is not a transfer. The terminal
types the file's **path**, escaped so the shell reads it as one word, and
follows it with a space. Several files become several words. That is how a
TUI such as OpenCode turns a dropped PNG into an attachment — it is parsing a
pasted path. iTerm escapes with backslashes
([its own issue #918](https://gitlab.com/gnachman/iterm2/-/issues/918)
concerns exactly which characters).

### Files

| File | Role |
|---|---|
| `crates/st-native/src/dnd.rs` | **Pure, headless-tested.** `shell_word(path)` → escaped path + trailing space. `shell_words(paths)` → all of them in drop order. `Flavor::{Posix, Windows}` picks the dialect; `Flavor::native()` is the build target's. |
| `crates/st-native/src/element.rs` | `on_drop::<gpui::ExternalPaths>` at the end of `install_listeners`. Focuses the grid, then `GridState::paste(Some(&text), cx)`. |
| `crates/st-native/src/lib.rs` | `pub mod dnd;` |

### Escaping rules (`dnd::needs_escape`)

- Safe, left alone: ASCII letters, digits, and `. _ / - + , : @ = %`.
- Everything else in ASCII gets a backslash: space, both quotes, `( ) [ ] { }`,
  `$ ~ # ! & ; | < > * ?`, backslash itself, and newline.
- **Non-ASCII is never escaped.** The shell takes UTF-8 as-is; escaping `é`
  would only corrupt it. `café.png` stays `café.png`.
- **Windows** (`Flavor::Windows`): `\` is a path separator and neither cmd
  nor PowerShell reads `\ `, so the word is double-quoted when it contains
  anything outside the safe set, and typed bare otherwise.

Backslashes rather than quoting on POSIX because a quoted path is harder for
a program reading stdin to recognise as a path, and every POSIX shell accepts
the backslash form.

### Why it goes through `paste`

`GridState::paste` → `prepare_paste` normalises newlines, wraps in
`ESC[200~ … ESC[201~` when `Modes::BRACKETED_PASTE` (2004) is on, and strips
any embedded `ESC[201~`. Routing the drop through it means a path can never
be read as keystrokes, and the program sees it the same way it sees ⌘V.

### What GPUI does for us

A macOS file drag reaches the window as `PlatformInput::FileDrop`
(`vendor/gpuix/zed/crates/gpui/src/window.rs`, the `FileDropEvent::Entered
{ paths }` arm), is promoted to an active drag of `ExternalPaths`, and
`FileDropEvent::Submit` fires every `on_drop::<ExternalPaths>` listener under
the pointer. gpuix registers no drop handling of its own, so nothing
competes for the event. `install_listeners` is generic over
`StatefulInteractiveElement`, which has the `on_drop` builder.

### Not done

- **Option-drag to upload** (iTerm's other drop mode, scp to the remote
  host). There are no remote sessions yet; nothing to upload to.
- **Drop onto the sidebar or headers.** The listener is on the grid element.
  A drop elsewhere in the window does nothing.
- **A drop cursor / highlight while dragging over.** GPUI supports
  `drag_over::<ExternalPaths>` styling; not wired.

---

## 3. Reordering tabs by dragging

### The wire was already there

`tab.reorder { tab, index }` has been in the protocol since 1.0,
`Workspace::reorder_tab` is implemented in
`crates/st-server/src/workspace/model.rs`, and
`crates/st-server/src/workspace/actor.rs` dispatches it. **Only the UI was
missing.** Nothing in this section touches Rust. (`tab.move`, which moves a
Tab to a *different* Session, is likewise implemented server-side and still
has no UI — see §5.)

### Files

| File | Role |
|---|---|
| `packages/app/src/state/tab-drag.ts` | **Pure.** `tabExtent`, `dropIndex`, `moveItem`, `previewOrder`, `REORDER_THRESHOLD`. 18 tests, no React, no store. |
| `packages/app/src/state/types.ts` · `reducers.ts` | `ui.tabDrag`, and the `tabDrag.begin` / `.to` / `.clear` actions. `pruneUi` drops a drag whose tab the next snapshot no longer has. |
| `packages/app/src/ui/TabStrip.tsx` | `Tab` owns the pointer plumbing; the strip owns the geometry and commits. |

### Why the drag is delta-based

The same constraint `state/layout.ts` documents for dividers: **gpuix reports
pointer positions in window coordinates but exposes no element bounds to
JavaScript.** `getElementBounds` exists only on
`@gpuix/react/dist/testing`, and `automation::track_own_bounds` is applied
solely to the native `<terminal-grid>` — chrome divs record nothing. So a tab
cannot ask where it or its neighbours are.

It does not need to. A reorder only cares how far the pointer has travelled
*since the press*, in slots: `round(delta / extent)`. The origin never
enters, so the banner, the session chip and the sidebar header cannot skew
it. `round`, not `trunc`, so a tab swaps once it is more than halfway across
its neighbour.

### The extents, and the one approximation

- **Vertical: exact.** `rowHeight + space.xs`, both tokens, both set by `Tab`.
  Every row is the same height.
- **Horizontal: `tabMaxWidth + gap`.** Tabs there are content-sized between
  `tabMinWidth` and `tabMaxWidth`, and text width is not knowable in JS. This
  is exact once titles are long enough to ellipsize — the crowded case. With
  short titles the real tabs are narrower, so the drag reads slightly low:
  the pointer travels a little further than the tabs it passes. It shows as
  drag **gain**, never a wrong drop, because the user steers by the live
  preview and releases when the gap is where they want it.

If gpuix ever exposes bounds to JS, `tabExtent` is the only function that
changes.

### Interaction details worth keeping

- **Nothing is sent until release.** `ui.tabDrag` is a local preview; the
  release fires one `tab.reorder`, and only if the index actually changed.
- **`REORDER_THRESHOLD` (4px)** gates the drag, so a plain click still just
  activates. Without it, a pixel of pointer drift between press and release
  would reorder the strip by accident.
- **The press is on the activate child, not the row.** The close `×` is its
  sibling; starting a drag from it would fight the button.
- **`onMouseDown` + `onMouseMove` on the same element** give it GPUI pointer
  capture, so moves and the release keep arriving after the pointer leaves
  the row. An ancestor cannot do this — `ui/drag.ts` explains why (filled
  elements occlude the hit test, so the frame is never "hovered").
- **The row's `onClick` still fires on release**, so dragging a tab also
  activates it. That is what a browser tab strip does.
- **`previewOrder` re-finds the tab by id** instead of trusting the stored
  `from`. A tab can close, or the server can reorder, mid-drag.
- **Identity stability matters**: `moveItem`/`previewOrder` return the
  *original* array for a no-op, and the reducer returns the same state for a
  `tabDrag.to` that does not change the slot, so `useSyncExternalStore` does
  not re-render the strip on every pointer move that does not change the order.

## 4. Chrome text is not prose

Pressing a tab started a text selection, and dragging it to reorder painted
the title blue. gpuix makes `<text>` selectable by default.

`userSelect: 'none'` is a real gpuix style prop
(`packages/native/src/style.rs`), and `selection_start_flag` in
`renderer.rs` treats it as "a selection may not **start** here". It
**inherits** to descendant text, so one declaration on a button covers its
label and every badge inside it.

Applied to: tab rows, the session chip (both layouts), the new-tab button,
`IconButton`, palette rows, menu rows, banner actions, and the four chrome
bars (sidebar header/footer, content header, title bar).

Two things this deliberately does **not** do:

- **The palette's `<input>` is untouched.** Inputs are a different element
  type with their own focus handles and editing; the flag is set on
  button-like containers, not at the app root, so text entry and selection
  inside the field are unaffected.
- **The terminal grid is unaffected.** Its selection is the native element's
  own, not gpuix text selection.

A `userSelect: 'none'` run still paints a highlight wash if a selection
started elsewhere sweeps across it. That is deliberate in gpuix (a browser
still finds such text with Ctrl+F) and is not a bug to chase.

## 5. Traps

- **Do not add a `+ 1` on the element side of any mouse path.** The encoder
  owns it. If a new report type is added, write the end-to-end bytes test
  first.
- **A green suite can pin a bug.** `report_cells_are_one_based` was the
  regression test for the wrong behaviour. When a test asserts a boundary
  convention (0- vs 1-based), also assert the resulting bytes.
- **The platform drag session cannot be exercised headlessly.**
  `simulateClick` aborts the process on macOS (see the layout skill), and
  there is no `simulateDrop`. The drop code path is unit-tested up to the
  text it types; the platform hop was verified by hand.
- **The addon is `dlopen`ed once per process.** After rebuilding
  `crates/st-native`, restart the client; a running window keeps the old
  behaviour and will look like the fix did not land. Chrome-only changes
  (§3, §4) need no rebuild, just a relaunch.
- **Do not reach for element bounds in chrome code.** `getElementBounds` is
  the testing surface. Derive geometry from tokens, and make the interaction
  delta-based so the origin cannot matter.
- **Check the wire before building a feature.** Tab reordering was a UI-only
  change because the protocol and server already had it. `tab.move` is in the
  same position today.

---

## 6. Verify

```bash
# Native (§1, §2), headless:
source scripts/env.sh
cd crates/st-native && cargo +1.97.1 test --lib 2>&1 | grep -E 'dnd::|wire_as_1_1|zero_based|test result'
#   7 dnd tests, the two report_cell tests, 171 passed

# Chrome (§3, §4), headless:
bun run typecheck && bun test 2>&1 | tail -4
#   287 passed, 18 of them in state/tab-drag.test.ts

# Rebuild only if the native crate changed (the addon loads once per process):
source scripts/env.sh
(cd crates/st-native && cargo +1.97.1 build --release)
cp crates/st-native/target/release/libst_native.dylib packages/native/superterminal-native.$ST_TRIPLE.node
pkill -f 'app.tsx'; ./scripts/run.sh --no-build
```

Then, by hand, in the new window — none of these can be exercised headlessly,
because the platform drag session and GPUI hit-testing are not simulable here
(`simulateClick` aborts the process on macOS; see the layout skill):

1. Run `opencode` in a tab. Click a session tab in its header → it switches.
2. Drag a PNG from Finder onto the grid → its escaped path plus a space
   appears; in OpenCode's prompt, an `[Image 1]` chip.
3. Drag a file whose name has a space → the space arrives as `\ `.
4. Drag a sidebar tab up or down → it moves; the others slide under it; the
   dragged row is outlined in accent.
5. Click a tab without moving → it just switches, no reorder.
6. Toggle to the horizontal strip (sidebar button) and drag a tab sideways.
7. Press a tab and drag → the title must **not** highlight blue.
