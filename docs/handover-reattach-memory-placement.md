# Handover: stable reattach, low memory, remembered window placement

Implementation brief for an AI agent working on `main` of
`sonnylazuardi/superterminal`. Three pieces of Windows-client feedback from
2026-09-11, each with a verified root cause, a decided design, and an
end-to-end verification recipe on the real infra (Windows client + WSL
Server). Nothing here has been implemented yet.

Read `CONTEXT.md` first and use its words (Server, Client, Surface, Replica,
Attach, Snapshot, Delta, Gap, Resync, Client State, Window Placement).

| | |
|---|---|
| Baseline | `main` @ `d3d4da2` (`CI: macos-15 + Xcode 26 for the 26.0 deployment target`) |
| Order of work | Part A (reattach) → Part B (memory) → Part C (placement). A is the bug fix; B and C are independent of each other and of A. |
| Do **not** | Change the wire format of `st-proto` (field *semantics* change in A, encoding does not). Restart the user's `--tcp 127.0.0.1:7171` daemon. Send keys or `WM_CLOSE` to any window you did not launch. Commit `vendor/gpuix`. |
| Done when | Every acceptance check in §A.7, §B.6, §C.6 passes; `bun run typecheck`, `bun test`, `cargo test --workspace`, and `cargo test` in `crates/st-native` (after `source scripts/env.sh`) pass; CONTEXT.md and ADR 0011 are landed; one PR per Part. |

---

## 0. The grill (questions answered on the user's behalf)

The user asked for the decision tree to be walked and answered without
blocking them. Facts were looked up in the tree and in the running daemon;
decisions are marked **adopted**.

### Part A — "reopen after a long time: session works, style is white or blank"

| # | Question | Answer |
|---|---|---|
| A-Q1 | Is the palette lost on the wire? | No. `Snapshot.styles` carries the whole Style Table and `build_snapshot` re-renders every cell (`crates/st-core/src/surface.rs:574-600`). The palette itself never leaves the Client (`crates/st-client-core/src/palette.rs`). |
| A-Q2 | Then why is everything the default foreground? | Cells reference a Style Table by index. When the Replica's table does not match the Server's, `get_or_default` maps unknown indices to `Style::DEFAULT` = fg `#d4d4d4` on an unpainted background (per the code map: `crates/st-client-core/src/runs.rs:141-148`, `palette.rs:197`). That *is* the "all white" screenshot. |
| A-Q3 | What desynchronises the table? | Three Server-side bugs that only show with **two subscribers on one Surface** (§A.1 R1–R3) plus one Client/Server bug that stops recovery (§A.1 R4). The user reproduces it because the installed exe (pid 22300, running since Sep 7) stays attached while a second instance is opened, and because `st` tool clients also Attach. |
| A-Q4 | Is "blank" the same bug? | Yes. Between the Gap and the forced reconnect (30 s, `crates/st-server/src/data/pump.rs:294`) the pane paints a stale or empty Replica; `with_slot` creates an empty Replica on any first message (`dataplane.rs:419-432`). |
| A-Q5 | Fix on the Client (tolerate) or the Server (be correct)? | **Adopted: Server-correct, Client-defensive.** Sequencing and style visibility become per-subscription on the Server; a duplicate Attach becomes a Resync; the Client keeps its Gap → Resync path and gains counters. |
| A-Q6 | Change the wire format? | **Adopted: no.** `Delta.since_seq` and `new_styles` already exist; only their *meaning* is fixed (per-subscription). Snapshot stays a full table. |
| A-Q7 | Should a Snapshot for one subscriber reset the Style Table? | **Adopted: no.** The table is append-only within a generation and resets only on overflow (>4096 styles), which already forces a Snapshot to everyone (`surface.rs:565-568`). |
| A-Q8 | Should the Server accept an Attach from an already-attached Client? | **Adopted: yes, as a Resync** (mode may change; `needs_snapshot = true`). `DataError "already attached"` goes away. |
| A-Q9 | Multiple Clients resizing one Surface: whose size wins? | Unchanged: last writer wins (`crates/st-server/src/data/conn.rs:415-417`, Q40). Out of scope here; note it in the PR. |
| A-Q10 | ADR? | **Adopted: yes**, `docs/adr/0011-per-subscription-sequencing.md` (wire *semantics*, hard to reverse, surprising, real trade-off vs a global sequence). |

### Part B — "super low memory, accurate rendering of all terminals and splits"

| # | Question | Answer |
|---|---|---|
| B-Q1 | What do we measure today? | Client `bun.exe`: 133 MB working set, 465 MB private bytes right after launch with 1 visible Pane. Installed exe: 60 MB / 496 MB. Server (debug build, 10 Surfaces, 18 h up): 87 MB RSS. Measured 2026-09-11 23:05 with `Get-Process` and `ps`. |
| B-Q2 | Where can the Client grow without bound? | (1) Replicas are never released: `forget` and `shrink_history_to` exist (`dataplane.rs:521-524`, `replica.rs:544-551`) and are never called from `st-native`; `docs/plan/04-client-native.md:108` specifies `shrink_to(1000)` on unmount. (2) The reader thread's event `Vec` is unbounded (`dataplane.rs:194,228`); it is only drained by `pump()` during a frame (`crates/st-native/src/conn.rs:70-88`), and Windows does not run frames while minimised (`gpui_windows/src/events.rs:240-259`). |
| B-Q3 | What is the 465 MB private? | Unknown; not attributable from outside. **Adopted:** attribute first (§B.2 step 1) before optimising. Suspects, in order: JSC heap reserve (Bun), D3D swap chain + shader cache, glyph atlas. |
| B-Q4 | Per-frame allocations? | `layout_viewport` allocates a `Vec<RowLayout>` and a `String` per cell every frame; `paint.rs:317` clones each `RunSpan` (code map). Bounded by viewport, but it is the hot path. **Adopted:** reuse buffers, measure with the existing `stats` prop. |
| B-Q5 | Server memory? | Dominated by scrollback: `terminal.scrollback_lines` default 10 000 (`docs/config-example.toml:69`). 168 cols × 10 000 rows × ~24 B ≈ 40 MB per busy Surface worst case. **Adopted:** keep the default, report per-Surface bytes in `st status`, and run the user's daemon as a **release** build (7171 is a debug binary today). |
| B-Q6 | What does "accurate rendering of all splits" mean here? | Every Pane of the visible Tab is mounted (`SurfaceHost.tsx:6`) and shares one Data Plane connection (`conn.rs:130-180`). Accuracy bugs seen so far are all Part A. **Adopted:** add a split-pane pixel test to the Windows recipe (§B.5) and treat any remaining artefact as a new ticket. |
| B-Q7 | Budget? | **Adopted:** Client ≤ 150 MB working set and ≤ 250 MB private after 1 h with 4 Panes; no growth > 5 MB/h while minimised; Server ≤ 10 MB + scrollback per Surface. Numbers to be confirmed by the baseline in §B.2. |

### Part C — "reopened window is in a different place / on a different monitor"

| # | Question | Answer |
|---|---|---|
| C-Q1 | What is remembered today? | Only `window.{width,height}` (`packages/app/src/state/client-state.ts:28-29`, `window-options.ts:63-80`). |
| C-Q2 | Why a different monitor? | gpuix always opens at `Bounds::centered(None, size, cx)` = centred on the **primary** display (`vendor/gpuix/packages/native/src/renderer.rs:1118-1122`); `WindowOptions` has no `x`/`y`/display (`renderer.rs:5472-5499`). |
| C-Q3 | Does GPUI support placement? | Yes: `WindowOptions.window_bounds: WindowBounds::{Windowed(Bounds), Maximized, Fullscreen}`, `display_id`, `cx.displays()`, `PlatformDisplay::uuid()/bounds()`, `window.bounds()`, `window.display(cx)`, `window.window_bounds()`, and a platform `on_moved` callback (`vendor/gpuix/zed/crates/gpui/src/platform.rs:356,867,1840-1880`; `app.rs:1321-1326`; `window.rs:2189,2462,2634`). |
| C-Q4 | Patch gpuix or upstream? | **Adopted: patch first** (`patches/0003-window-placement.patch`, applied by `just vendor-patch` like the two existing patches), then open an upstream PR to `remorses/gpuix`. |
| C-Q5 | Move events or polling? | **Adopted: polling.** Sample `getWindowPlacement()` on every resize event and every 2 s; the Client State persister already debounces and flushes on exit. A `windowMoved` event is a nice-to-have in the upstream PR. |
| C-Q6 | How is a display identified across reboots? | **Adopted:** by `uuid()` when the platform gives one, else by the display's bounds. Restore only if the remembered origin's window centre lies inside a currently connected display; otherwise fall back to today's behaviour (centred on primary). |
| C-Q7 | Maximised / fullscreen? | **Adopted:** remember `maximized: true` and restore as `WindowBounds::Maximized` on that display; never persist fullscreen (restore as maximised). |
| C-Q8 | Mixed DPI? | GPUI bounds are logical pixels per display and gpuix already enables per-monitor DPI on Windows (`renderer.rs:1109`). Test it (§C.6), do not special-case it. |

---

## Part A — Stable reattach

### A.1 Root causes (verified in the tree on 2026-09-11)

**R1 — `since_seq` is global, subscriptions are not.**
`crates/st-core/src/surface.rs:509-570` `Surface::flush`: one `seq` per
flush for every subscriber, and every Delta gets `since = seq - 1`.
`crates/st-core/src/publisher.rs:481-482` skips a subscription whose
`pending` is empty. So whenever *one* subscriber receives a frame while
*another* has nothing pending (the common case: a fresh Attach's Snapshot
while the older Client is idle), the idle Client's Replica stays at `seq-1`,
its next Delta says `since_seq = seq`, and `Replica::apply_delta`
(`crates/st-client-core/src/replica.rs:404-412`) reports a **Gap**.

**R2 — one Style Table per Surface, reset by any Snapshot.**
`surface.rs:574-577` `build_snapshot` calls `self.styles.reset()`
(new generation) for *one* subscriber. Every other subscriber still holds
indices of the previous generation. `surface.rs:634` `take_new()` is one
global flush window, so a subscriber that skipped a flush never receives the
style entries another subscriber's Delta consumed. Either way the other
Client paints unknown indices as `Style::DEFAULT` → **all white**.

**R3 — Resync is dead-ended.**
`crates/st-client-core/src/dataplane.rs:443-460` `request_snapshot` re-sends
`Attach{want_snapshot: true}` while still attached.
`crates/st-server/src/data/conn.rs:311-330` `on_attach` →
`publisher.rs:289-292` `attach` returns `false` for an existing subscriber
→ `DataError "already attached"`, **no Snapshot**. The Replica never
advances, never Acks, and 30 s later the Server closes the connection
(`crates/st-server/src/data/pump.rs:294`). The Client reconnects,
`reattach_all` gets a Snapshot, that Snapshot resets the table (R2) and
Gaps the *other* Client (R1) — a ping-pong.

**R4 — double Attach on reconnect.**
`crates/st-native/src/element.rs:383-392` clears `attached` on
`Disconnected`, so `ensure_connection` re-Attaches on the next frame in
addition to `reattach_all` (`dataplane.rs:971-992`). Harmless noise today,
a real Resync after A.2 step 3 — must be removed.

**Evidence.** `/tmp/st-tcp-test/daemon.log` (the user's 7171 daemon):

```
2026-09-10T15:03:17Z conn{id=173 plane="data"} ...           # installed exe attaches surface 15
2026-09-11T14:59:05Z control client connected client=176     # second client launched
2026-09-11T14:59:08Z WARN no Ack for 30 s; closing the connection client=173 surface=15
2026-09-11T15:02:42Z conn{id=184 plane="data"} ...           # second client's data plane
2026-09-11T15:03:32Z WARN no Ack for 30 s; closing the connection client=184 surface=1
2026-09-11T15:04:02Z WARN no Ack for 30 s; closing the connection client=191 surface=1
2026-09-11T15:05:16Z WARN no Ack for 30 s; closing the connection client=193 surface=1
```

Screenshots from the user (2026-09-11): btop pane (surface 2) and the
opencode pane (surface 1) painted entirely in default fg; the htop pane
beside btop (surface 10) still coloured.

### A.2 Design (adopted)

1. **Per-subscription `since`.** `Emission` carries `since: Seq` =
   `sub.last_sent_seq` at flush time; `Surface::flush` passes it to
   `build_delta`. A subscription that skipped flushes therefore gets a Delta
   whose `since_seq` equals the last frame *it* received, and its per-sub
   `pending.dirty` already accumulated every row since then.
2. **Per-subscription style visibility.** Replace the global flush window
   (`mark_all_flushed`, `take_new`, `rollback_flush_window` in
   `crates/st-core/src/style_table.rs`) with two fields on `Subscription`:
   `styles_generation: u32`, `styles_sent: u16`. `build_delta(sub)` sends
   `new_styles = table[styles_sent..len]` and sets `styles_sent = len`;
   `build_snapshot(sub)` sends the whole table and sets both fields.
   `build_snapshot` **no longer resets** the table. Overflow (interning
   past `STYLE_TABLE_CAP`) resets + bumps the generation exactly as today
   and `force_snapshot_all()` follows; a subscription whose
   `styles_generation` differs from the table's is treated as
   `needs_snapshot`.
3. **Duplicate Attach = Resync.** In `publisher.rs` add
   `resync(client, mode, now) -> bool` (sets `mode`, `needs_snapshot = true`,
   clears `pending`, resets the stall fields). `data/conn.rs::on_attach`
   calls `attach`, and if that returns `false`, calls `resync` instead of
   `send_error`. Log at `debug` with `resync = true`.
4. **Client hygiene.** `DataPlaneHandle::is_attached(surface) -> bool`;
   `element.rs::ensure_connection` uses it instead of the local `attached`
   flag after a reconnect. Keep the local flag for the `attached` readable
   prop.
5. **Observability.** `Replica`/`DataPlane` counters `gaps`, `resyncs`,
   `snapshots`; expose through the `stats` readable prop and through
   `st status` (Server side: `resyncs` next to `snapshots`). A Gap that is
   not healed by a Snapshot within 2 s logs at `warn` with surface id and
   both seqs.
6. **Windows background.** `packages/app/src/platform/window-options.ts`
   `resolveBackground`: add an explicit `platform.isWindows` branch returning
   `'opaque'` for `'auto'` and `'blurred'` (`'transparent'` stays
   `'transparent'`). Unit test the table.

### A.3 Files to touch

```
crates/st-core/src/publisher.rs      Subscription fields, resync(), flush() emits since
crates/st-core/src/surface.rs        flush(), build_snapshot(), build_delta() per-sub styles
crates/st-core/src/style_table.rs    drop the flush-window API, keep generation + overflow
crates/st-server/src/data/conn.rs    on_attach → resync path; status counters
crates/st-client-core/src/dataplane.rs  is_attached(), counters, gap watchdog log
crates/st-client-core/src/replica.rs    counters
crates/st-native/src/element.rs      ensure_connection uses is_attached; stats
crates/st-cli/src/...                st status prints resyncs
packages/app/src/platform/window-options.ts (+ .test.ts)
docs/plan/02-protocol.md §6          document per-subscription since/new_styles + Resync
docs/adr/0011-per-subscription-sequencing.md  (draft exists, set Status: Accepted)
```

### A.4 Tests to add (unit)

- `publisher.rs`: two subscribers; C1 idle across two flushes while C2
  receives frames; C1's next Delta has `since_seq == C1.last_sent_seq`.
- `surface.rs`: two subscribers; Snapshot for C2 does **not** change the
  indices C1 holds; a style interned during C2's Delta reaches C1 in C1's
  next Delta; overflow forces a Snapshot to both.
- `surface.rs`: `Attach` twice from one client → second returns a Snapshot
  frame, no `DataError`.
- `st-server` `data/conn.rs` test: duplicate Attach over a real socket yields
  `Snapshot`, and switching `mode` on the second Attach is honoured.
- `dataplane.rs`: Gap → `request_snapshot` → Snapshot arrives → Replica seq
  and styles match; `gaps == 1`, `resyncs == 1`.
- `dataplane.rs`: reconnect → exactly one `Attach` per attachment (pin R4).
- `window-options.test.ts`: Windows rows of the background table.

### A.5 End-to-end verification (real infra, from WSL)

Never touch the user's window (pid of `superterminal.exe` started 9/7,
and any `bun.exe` you did not start). Use the 7172 test daemon and the
driver in `C:\Users\sonny\AppData\Local\Temp\st-test\st-test.ps1`
(memory note `windows-client-testing` has the full recipe).

1. Build: `cargo build --release -p st-server -p st-cli` in WSL; rsync
   `crates/` (exclude target, dist) and `packages/app/src/` to
   `C:\Users\sonny\superterminal`; `cmd.exe /c C:\Users\sonny\build-native-release.bat`;
   copy the `.dll` over both `.node` files (see memory note
   `native-crate-testing`).
2. Start a fresh test daemon (release binary) on 7172 with
   `--no-idle-exit`, `RUST_LOG=info`, log to a file.
3. Launch **two** test clients against 7172 with separate `LOCALAPPDATA`
   (the driver's `launch` twice with different `st-test` dirs). Both show
   the same Tab.
4. Paint colour into the Surface without typing:
   `printf '\e[31mred \e[32mgreen \e[44m blue-bg \e[0m plain\n' > /dev/pts/N`
   (find N via `st ls --tcp 127.0.0.1:7172` → pid → `ps -o tty`).
5. Wait 90 s. Assert: daemon log has **no** `no Ack for 30 s`; both
   screenshots (`st-test.ps1 shot A`, `shot B`) show red/green/blue;
   `stReadProp(id,'stats').gaps == 0` in both.
6. Close client B, reopen it (this is the user's scenario). Repeat step 5.
7. Minimise client A for 5 min while `yes | head -c 50M > /dev/pts/N`
   runs; restore. Assert colours and no `no Ack`.
8. Run `st probe`/`st ls` (Tool clients Attach too) against 7172 while both
   clients are up; assert no Gap counters move in the clients.

### A.6 Rollout to the user's daemon

The user's sessions live in the **debug** daemon on 7171 (pid 3080980,
`/tmp/st-tcp-test`). Do not restart it yourself. Tell the user: start the
new **release** daemon on 7171 after closing all Clients; PTYs die with the
old daemon (no persistence across restarts, `crates/st-server/src/persist.rs:7`).

### A.7 Acceptance

- Two Clients on one Surface for 30 min with colour output and idle periods:
  zero `no Ack` lines, zero Gaps, identical colours in both.
- Reopening a Client while another stays attached never changes what the
  other paints.
- `st status` shows `resyncs` and it stays 0 in the above runs.

---

## Part B — Low memory, accurate rendering

### B.1 Facts

| Process | Working set | Private bytes | Notes |
|---|---|---|---|
| dev client `bun.exe` (1 Pane) | 133 MB | 465 MB | 46 threads, right after launch |
| installed `superterminal.exe` | 60 MB | 496 MB | up 4 days |
| Server 7171 (debug) | 87 MB RSS | | 10 Surfaces, scrollback 10 000 |

Client structure: one Data Plane connection per socket path shared by all
grids (`crates/st-native/src/conn.rs:130-180`); one Replica per Surface
ever attached, never dropped; history cap 10 000 rows per Replica
(`replica.rs:45-51`), filled only by History fetches; `RunCache` LRU 256
(`element.rs:218`); Sprites are geometry, not textures (ADR 0010).

### B.2 Steps

1. **Attribute before optimising.** Add `DEBUG=st:mem` logging every 30 s
   in `packages/app/src/app.tsx`: `process.memoryUsage()` (`rss`,
   `heapUsed`, `heapTotal`, `external`) plus the sum of
   `stReadProp(id,'stats').replicaBytes` (new field, see step 2). Add a
   PowerShell sampler `C:\Users\sonny\AppData\Local\Temp\st-test\mem-sample.ps1`
   that appends `WS,Private,GPU-shared` (from `Get-Process` and
   `Get-Counter '\GPU Process Memory(*)\Shared Usage'`) every 30 s for a
   named pid. Produce the baseline table for: launch, +1 h idle, +1 h
   minimised with a busy Surface, 4 Panes visible, after switching through
   10 Tabs. Record it in `docs/perf/memory-2026-09.md`.
2. **`replicaBytes` stat.** `Replica::approx_bytes()` = visible + history
   rows × cells × `size_of::<PackedCell>()` + style table; surfaced through
   `stats`.
3. **Release Replicas.** In `element.rs` `destroy()`:
   `handle.shrink_history_to(surface, 1000)` (the number from
   `docs/plan/04-client-native.md:108`). On `Exited` + Tab close (the
   control plane already knows the Surface is gone) call `handle.forget`.
   Add a `forgotten` counter to `stats`.
4. **Bound the event queue.** `dataplane.rs` `push_event`: cap at 1024,
   coalesce consecutive `Bell` of the same Surface and drop the oldest
   non-`Connected`/`Disconnected`/`Detached`/`Exited` event when full.
   Replica state is the source of truth; events are wake-ups.
5. **Per-frame allocations.** `runs.rs`: keep `Vec<RowLayout>` and the
   per-cell `String` as reusable buffers on the element state; take
   `RunSpan` by reference in `paint.rs`. Measure `stats.frameMs` p50/p99
   before and after on the btop Tab (the existing nightly perf harness in
   `scripts/perf-compare.ts` can be reused).
6. **Bun heap.** If step 1 shows `heapTotal` > 100 MB, try
   `bun build --compile --smol` for the packaged exe and re-measure.
7. **Server.** `st status` prints per-Surface `scrollback rows / bytes`;
   README/WINDOWS.md tell users to run a release daemon. Do not change the
   default `scrollback_lines`.

### B.3 Files to touch

```
crates/st-client-core/src/{dataplane.rs,replica.rs,runs.rs}
crates/st-native/src/{element.rs,paint.rs,conn.rs}
crates/st-server/src/{control/*status*,supervisor.rs}   scrollback bytes
crates/st-cli/src/...                                   st status output
packages/app/src/app.tsx                                st:mem logging
docs/perf/memory-2026-09.md                             baseline + after
```

### B.4 Tests

- `dataplane.rs`: push 5 000 events with no pump → queue length ≤ 1024 and
  the last event is retained.
- `replica.rs`: `shrink_history_to(1000)` after 10 000 fetched rows frees the
  rows and keeps the visible screen.
- `element.rs` (Linux headless test renderer): destroy → `replicaBytes`
  drops; `forget` after Exited.

### B.5 Accurate rendering check (Windows, 7172)

Open a Tab with a 2×2 split (Split Right, then Split Down in both), write a
distinct colour bar and a box-drawing frame into each of the four Surfaces
via `/dev/pts/N`, screenshot, and check: each Pane's frame is closed (no
seam), colours are per Pane, no Pane is blank, and after a window resize
every Pane re-renders at its new cell count (`st ls` shows the new
cols×rows). Anything else is a new ticket, not part of this handover.

### B.6 Acceptance

- Baseline table filled in and committed.
- After 1 h minimised with a busy Surface: private bytes growth < 5 MB.
- 4 Panes visible for 1 h: working set ≤ 150 MB, private ≤ 250 MB (revise
  with the baseline if the JSC reserve alone exceeds this; say so in the PR).
- `stats.frameMs` p99 on btop not worse than before step 5.

---

## Part C — Remembered window placement

### C.1 Facts

- Client State today: `window: {width, height}`
  (`packages/app/src/state/client-state.ts`), sampled from the resize
  action (`reducers.ts:431`) and flushed on exit (`app.tsx:138-157`).
- gpuix opens every window centred on the primary display
  (`vendor/gpuix/packages/native/src/renderer.rs:1118-1122`) and exposes
  only `getWindowSize()` (`index.d.ts:46`).
- GPUI can open at `WindowBounds::Windowed(Bounds{origin,size})` or
  `Maximized` on a given `display_id`, list displays with `uuid()` and
  `bounds()`, and read a window's bounds and display at runtime.

### C.2 Design (adopted)

1. **gpuix patch `patches/0003-window-placement.patch`.**
   - `WindowOptions` gains `x?: number`, `y?: number`,
     `display?: { uuid?: string; bounds?: {x,y,width,height} }`,
     `maximized?: boolean`.
   - In both `init_macos` and `init_threaded`: if `x`/`y` are set, pick the
     display by `uuid` (fallback: the display whose `bounds` contain the
     window centre `(x + w/2, y + h/2)`); if none matches, fall back to
     `Bounds::centered`. Build `WindowBounds::Maximized(bounds)` when
     `maximized`, else `Windowed`.
   - New napi `getWindowPlacement(): { x, y, width, height, maximized,
     fullscreen, display: { uuid?: string, bounds } }` reading
     `window.window_bounds()`, `window.bounds()`, `window.display(cx)`.
   - `index.d.ts` and the React `render()` option types updated.
2. **Client State v2.** `window` becomes
   `{ width, height, x?, y?, maximized?, display?: { uuid?, bounds? } }`.
   Parsing stays field-lenient (a bad `x` drops placement, keeps size).
   Version stays 1 (additive); a v1 file loads unchanged.
3. **Sampling.** `packages/app/src/state/window-placement.ts` (pure,
   testable like `window-size.ts`): `readWindowPlacement(renderer)`.
   `app.tsx` samples on every resize action and on a 2 s interval, and
   dispatches `ui/window-placed` only when something changed; the existing
   persister debounce and exit flush do the rest. Never persist
   `fullscreen: true` (store `maximized: true` instead).
4. **Restore.** `buildWindowOptions` passes `x`, `y`, `display`,
   `maximized` from Client State. Validation on the JS side: drop the
   origin if the remembered `display.bounds` no longer contains the window
   centre *and* no `uuid` matches — but the native side re-validates anyway
   (a display can disappear between reads).
5. **Config.** No new Config keys; Config never wins over Client State (ADR
   0008).

### C.3 Files to touch

```
patches/0003-window-placement.patch      (vendor/gpuix renderer.rs, index.d.ts, react render options)
justfile                                 nothing: vendor-patch already loops over patches/*.patch
packages/app/src/state/{client-state.ts,window-placement.ts,reducers.ts,types.ts} (+tests)
packages/app/src/platform/window-options.ts (+test)
packages/app/src/app.tsx                 sampling + options
docs/DEV.md, docs/WINDOWS.md             one paragraph each
CONTEXT.md                               Window Placement (already added)
```

### C.4 Tests

- `client-state.test.ts`: v1 file loads; v2 with a bad `x` keeps size;
  `maximized` round-trips.
- `window-options.test.ts`: remembered placement whose centre is outside
  every given display → no `x`/`y` in the options.
- `window-placement.test.ts`: a throwing renderer → `null`, never a default.

### C.5 Upstream

After the patch works, open a PR against `remorses/gpuix` with the same
change (plus an optional `windowMoved` event) and record the PR link in
`docs/PINS.md`.

### C.6 End-to-end verification (Windows, 7172)

1. Launch a test client; move it to the secondary monitor via the driver
   (`SetWindowPos` on the hwnd is fine for a window you launched) and
   resize it; close it with `st-test.ps1 close`.
2. Relaunch: assert `st-test.ps1 measure` reports the same origin/size and
   the same monitor (`MonitorFromWindow`).
3. Maximise on the secondary monitor, close, relaunch: opens maximised
   there.
4. Disconnect the secondary monitor (or edit `client.json` to an off-screen
   origin), relaunch: opens centred on the primary, no crash, warning line
   in the log.
5. Mixed DPI (laptop 125 % + external 100 %): steps 1–3 on both.

### C.7 Acceptance

- Reopening restores position, size, monitor and maximised state on
  Windows and macOS; a missing monitor degrades to today's behaviour.
- `bun test` covers the three files above; the patch applies cleanly in CI
  (`just vendor-patch` on Linux and macOS jobs).

---

## Appendix — commands and pitfalls

- `source scripts/env.sh` before any `cargo` in `crates/st-native`.
- `cargo test --workspace` never builds st-native (workspace `exclude`).
- Windows build/test chain, DPI trap, drive hook and the "don't touch the
  user's window" rule: memory notes `windows-client-testing`,
  `native-crate-testing`, `windows-packaging-chain`.
- `gh` on this box has an expired token; `git push` over SSH works; open PRs
  after `gh auth login` or hand the branch to the user.
- Commit messages end with the attribution lines the session provides.
