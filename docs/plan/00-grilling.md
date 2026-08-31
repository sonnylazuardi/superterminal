# 00 — Grilling Session (self-interview)

Format: each question is asked, a recommended answer is given, and the decision is **adopted** so downstream documents can rely on it. Questions are ordered so that each depends only on earlier ones. Facts were looked up (gpuix source, Bun 1.4.0 release notes, the Superlogical demo video); the decisions are ours.

Reference material used:
- Superlogical demo (Mitchell Hashimoto, 2026‑08‑28): client/server multiplexer, native macOS chrome, sessions, tabs, native scrollbar, "half a dock bounce" reconnect, selection preserved across client relaunch.
- gpuix 0.6.0 (`remorses/gpuix`): React → JSON mutation batch → Rust `RetainedTree` → GPUI. `CustomElement`/`CustomElementFactory` traits, `CustomElementRegistry::register()` (Rust-only), `canvas` element *planned, not implemented*. GPUI is a path dependency on a vendored Zed checkout. Prebuilt `.node` for darwin‑arm64, linux‑x64‑gnu, win32‑x64‑msvc.
- Bun 1.4.0 (2026‑08‑20): Node‑API supported, `Bun.Terminal` PTY in `Bun.spawn`, `bun build --compile --asset`, `bun --hot`, JSC‑native `bun:ffi`.
- Local toolchain: bun 1.4.0, cargo 1.96.0, node present. Dev machine is WSL2 (Linux). Project dir is empty.

---

## A. Product scope

### Q1. What is "superterminal", in one sentence?
**Recommended:** A GPU‑rendered, native‑feeling terminal *client* whose terminals live in a persistent local server (multiplexer), so processes survive the window closing and the client reconnects in well under a second — i.e. the Superlogical demo's slice, built on Rust + gpuix + Bun.
**Adopted.** Everything below serves this sentence. Anything not needed for it is out of v1.

### Q2. Who is the user for v1?
**Recommended:** Ourselves (the developer building it) on Linux (WSL2/Wayland or X11) and macOS. Not the general public. That lets us skip installers, auto‑update, and signing until M6.
**Adopted.**

### Q3. Which platform is primary?
**Recommended:** Linux is primary for *development* (that's the dev box; gpuix supports Vulkan/X11/Wayland); macOS is primary for *look and feel* (blurred window, transparent titlebar, traffic lights come from the `blurred-window.tsx` example and are macOS features). Windows is explicitly deferred: ConPTY re‑encodes escape sequences (Bun docs), and gpuix's win32 build is least exercised.
**Adopted.** CI builds all three; only Linux+macOS must pass for a milestone to close.

### Q4. Must v1 feel like "just a terminal" (no modes, no prefix key) like Superlogical, or is a tmux‑style modal UI acceptable?
**Recommended:** Non‑modal, exactly as the demo argues: normal terminal on open, native tabs, native scrollbar, GUI command palette. No prefix key. Keyboard shortcuts are ordinary app shortcuts (⌘/Ctrl+T, ⌘/Ctrl+K, …).
**Adopted.** This is a product constraint → CONTEXT.md and ADR‑0001.

### Q5. What is explicitly OUT of v1?
**Recommended out:** remote hosts over SSH (protocol must allow it; no implementation), split panes, web client, Windows support, themes/config UI beyond a TOML file, ligatures, images (Sixel/Kitty), search in scrollback, AI features, plugins.
**Adopted.** Each of these gets a one‑line "hook left for it" note in the architecture doc so nobody has to redesign for them.

---

## B. Architecture shape

### Q6. Client/server, or single process?
**Recommended:** Client/server from day one. Persistence across client relaunch (the demo's core trick) is impossible otherwise, and retrofitting a server later rewrites the terminal core's ownership model. The server is a per‑user daemon (`superterminald`) on a Unix domain socket.
**Adopted.** ADR‑0002.

### Q7. Where does the terminal state machine (VT parser + grid) run — server, client, or both?
**Recommended:** The **server owns the authoritative terminal state machine** (one per Surface). The **client keeps a replica grid** it can render from without the server (so scrolling/selection are local and instant). The server pushes *deltas* (dirty rows / cells) to each attached client; a fresh attach receives a *snapshot*. This is Mitchell's "N replica distributed terminal state machines" design in our words.
**Adopted.** Terms: *Surface*, *Replica*, *Snapshot*, *Delta* → CONTEXT.md. ADR‑0003.

### Q8. Do we write our own VT parser/grid or use a library?
**Recommended:** Use `alacritty_terminal` (Rust, mature, damage tracking, used by Zed's `terminal` crate) for the **server‑side authoritative** state machine. Write our own small **replica grid** (Vec of rows of packed cells + scrollback ring) on the client, because alacritty's `Term` cannot be driven by deltas. Do *not* use libghostty‑vt (C ABI via Zig; adds a toolchain and Zig is unfamiliar in this stack) for v1 — leave the VT engine behind a trait so it can be swapped.
**Adopted.** ADR‑0004.

### Q9. Which process owns the PTY?
**Recommended:** The Rust server, using `portable-pty` (or `rustix`+`openpty`). Not `Bun.Terminal`: the server has no JS in it, and the whole point is that terminals outlive the client. `Bun.Terminal` is noted as the fallback for a future "serverless dev mode" only.
**Adopted.**

### Q10. What is Bun's job, then?
**Recommended:** Bun is the **client app host**: it runs the React/TSX UI through `@gpuix/react` (Node‑API into our native module), provides the dev loop (`bun --hot`), the test runner (`bun test`), the scripts, and the single‑file build (`bun build --compile --asset` to embed the `.node`). Bun also runs the *control‑plane* socket client (`Bun.connect` on the Unix socket) for JSON control messages (create tab, list sessions, …).
**Adopted.** Bun is never on the cell‑rendering hot path.

### Q11. How does the client render the cell grid — React elements, or a native element?
**Recommended:** A **native gpuix custom element** (`<terminal-grid>`) implemented in Rust via `CustomElement`/`CustomElementFactory`. Rendering 80×50 = 4 000 cells (or 200×100 = 20 000) as `<div>/<text>` through JSON `applyBatch` every frame would be catastrophic (gpuix's own benchmark: 592 B/element retained, ~30 ms to apply 220 k ops; an 8 ms budget cannot absorb thousands of `setText` per frame). The custom element paints glyph runs directly with GPUI (`paint_quad`, shaped text runs), same approach as Zed's `terminal_view`.
**Adopted.** ADR‑0005. The React tree only holds chrome: tabs, session switcher, palette, one `<terminal-grid surfaceId=…>` per visible tab.

### Q12. gpuix's registry is Rust‑only and lives inside `gpuix-native`'s `GpuixRenderer::init` (`with_defaults()`); plugin loading is "phase 2, planned". Fork, vendor, or wait?
**Recommended:** **Vendor gpuix as a git submodule + a small patch** (or a fork branch) that adds one hook: `GpuixRenderer::init` accepts extra factories (or a `register_factory` napi method called before `init`). Build our own Node‑API module `@superterminal/native` from a crate that depends on `gpuix-native` (it is `crate-type = ["cdylib","rlib"]`) and registers `TerminalGridFactory`. Upstream the hook as a PR; if merged, the patch disappears.
**Adopted.** ADR‑0006. Consequence: we build GPUI from the vendored Zed checkout (long first build, ~10–20 min; cached afterwards). The plan budgets for this.

### Q13. How does the native `<terminal-grid>` element receive cell data — through React props, or directly?
**Recommended:** **Directly.** The native module owns a *data‑plane* connection to the server (binary framing), keeps replicas per attached Surface, and the element paints from the replica. React only passes `surfaceId` and style props. Deltas never touch JS.
**Adopted.**

### Q14. One multiplexed socket connection or two (control + data)?
**Recommended:** **Two connections, two codecs.** Control plane: Bun ↔ server, newline‑delimited JSON over the Unix socket (debuggable with `socat`). Data plane: Rust native module ↔ server, length‑prefixed binary frames (`postcard`/hand‑rolled). Ordering is naturally safe: JS creates a Surface via control and only then renders `<terminal-grid surfaceId>`, which triggers the data‑plane `Attach`.
**Adopted.** ADR‑0007. Alternative (single Rust‑owned connection bridged to JS via napi events) is recorded as the fallback if two‑connection races appear.

### Q15. Serialization for the binary data plane?
**Recommended:** Hand‑specified little‑endian frames with a `u32 len | u16 type | payload` header, payload structs encoded with `postcard` (serde, no schema files, tiny). Versioned by a `Hello` handshake. Not protobuf/flatbuffers (toolchain weight), not JSON (cell deltas are hot).
**Adopted.**

### Q16. What does a Delta contain?
**Recommended:** Per Surface: a monotonically increasing `seq`, the list of **dirty rows** (full row content for each dirty visible row; rows are small and row‑granularity avoids per‑cell bookkeeping), cursor state, mode flags (alt‑screen, mouse mode, bracketed paste, cursor shape), title, and a `scrollback_appended: u32` count. Scrollback history rows are pulled lazily by the client (`FetchHistory{from,count}`) rather than pushed.
**Adopted.** Cells are packed: `u32 codepoint | u16 style_idx | u8 flags` with a per‑Surface style table (fg/bg/attrs) interned by the server; the client mirrors the table.

### Q17. Where does UI state (tabs, sessions, per‑surface viewport, selection) live?
**Recommended:** In the **server** (a *Workspace* document: Sessions → Tabs → Surfaces, plus per‑Surface `view_state {scroll_offset, selection}`), because the demo shows the *selection surviving a client relaunch*. Client keeps a projected copy; edits go through control messages and are echoed back. Client‑only state: window geometry, focus, palette query text.
**Adopted.**

### Q18. Persistence across *server* restarts (reboot)?
**Recommended:** Not in v1. Processes die with the server anyway. We do persist the Workspace layout (sessions, tabs, cwd of each surface) to `$XDG_STATE_HOME/superterminal/workspace.json` on change, so a fresh server recreates the *shape* (new shells in the same cwds). Scrollback is not persisted.
**Adopted.**

---

## C. Sessions, tabs, surfaces

### Q19. Vocabulary — what are the nouns?
**Recommended:** *Server* (daemon), *Client* (GUI process), *Workspace* (all of one user's state on one server), *Session* (a named group of Tabs; the demo's "Demo"/"Work"), *Tab* (an ordered item in a Session; holds exactly one Surface in v1 — splits later would make it hold a layout tree), *Surface* (one PTY + one authoritative terminal state machine), *Replica* (client‑side copy of a Surface's grid), *Attach* (client subscribes to a Surface). Avoid: "pane", "window" (for surfaces), "terminal" (ambiguous), "buffer".
**Adopted.** → CONTEXT.md.

### Q20. Does a Session have its own window, or does one window switch between Sessions?
**Recommended:** One window; the tab bar shows the active Session's tabs and a Session chip at the far left (as in the demo). Switching Session swaps the tab strip. Multi‑window is deferred.
**Adopted.**

### Q21. What happens to a Surface when its tab is closed vs. when the client quits?
**Recommended:** Closing a tab **kills the Surface** (SIGHUP the process group, then destroy). Quitting the client **detaches** only; the server keeps everything running. Closing the last tab of a Session deletes the Session (unless it's the last Session, which is re‑seeded with a fresh tab).
**Adopted.**

### Q22. What does the server do when the *process* in a Surface exits?
**Recommended:** Surface enters `Exited{code}`; the grid stays readable; the tab shows a subtle "exited" badge; pressing Enter or clicking closes it. No auto‑close (tmux's `remain-on-exit off` loses the last output, which is a common complaint).
**Adopted.**

---

## D. Input, rendering, UX details

### Q23. Keyboard input path?
**Recommended:** GPUI key events land in the `<terminal-grid>` custom element (it holds a GPUI `FocusHandle`). The element encodes keys to VT bytes **in Rust** (via `alacritty_terminal`‑style key tables or our own encoder, incl. Kitty keyboard protocol later) and sends `Input{surface, bytes}` on the data plane. JS never sees terminal keystrokes, except app‑level shortcuts which the element declines and GPUI dispatches up to React (`onKeyDown` on the app root).
**Adopted.**

### Q24. Mouse, selection, copy/paste?
**Recommended:** Selection is computed client‑side on the Replica (instant), then sent as `SetSelection` so it persists. Copy uses GPUI clipboard; paste is bracketed when the Surface's mode says so. Mouse‑mode reporting is forwarded to the app when the program requests it (`mode.mouse != none`), Shift overrides to select.
**Adopted.**

### Q25. Native scrollbar and scrollback?
**Recommended:** The element exposes its content height (rows + history) to GPUI and renders a GPUI scrollbar (Zed has a `Scrollbar` component; gpuix has `ScrollHandle`). Scrolling is purely local on the Replica; history rows beyond what's cached are fetched in 1 000‑row pages. Auto‑scroll to bottom on output when already at bottom.
**Adopted.**

### Q26. Fonts and glyphs?
**Recommended:** One monospaced font family from config (default: system mono — `Menlo` on macOS, `DejaVu Sans Mono`/`JetBrains Mono` if present on Linux). Shape per **run of same‑style cells** using GPUI's text system (`shape_line`) and cache shaped runs by `(text, style)`; measure cell width from `'M'` advance; render wide chars as 2 cells; no ligatures in v1. Emoji via system fallback.
**Adopted.**

### Q27. Frame budget and repaint policy?
**Recommended:** Repaint only when a Delta arrives, the cursor blinks, or the user interacts (GPUI `cx.notify()`). Coalesce Deltas to at most one repaint per frame. Targets: attach‑to‑first‑paint < 100 ms warm; 60 fps while `cat` of a 100 MB file runs; input‑to‑glyph latency < 1 frame locally. The server throttles Deltas to ~120 Hz per client and always sends a coalesced final state.
**Adopted.**

### Q28. Window chrome?
**Recommended:** Reuse `blurred-window.tsx` settings verbatim on macOS: `titlebarTransparent`, `windowBackground: 'blurred'`, `trafficLightX/Y`, custom tab strip drawn in React with left padding for traffic lights. On Linux fall back to `'transparent'` if the compositor supports it, else `'opaque'`, chosen at startup by a probe/config flag.
**Adopted.**

### Q29. Command palette and keybindings?
**Recommended:** Palette is React (`<anchored>` overlay + `<input>` + list). Commands are a typed registry in TS (`id, title, shortcut, run()`), same registry drives the keybinding table. v1 commands: New Tab, Close Tab, Next/Prev Tab, New Session, Switch Session, Rename Session, Toggle Vertical Tabs, Copy, Paste, Clear Scrollback, Reconnect, Quit.
**Adopted.**

---

## E. Reliability, process model, dev experience

### Q30. Server lifecycle: who starts it?
**Recommended:** Client auto‑spawns `superterminald` if the socket is absent (`Bun.spawn`, detached, `unref`), then connects with a 3 s retry loop. The server exits when idle for N minutes *and* zero Surfaces exist (never while processes run). One server per user per `$XDG_RUNTIME_DIR`; a lockfile prevents duplicates.
**Adopted.**

### Q31. Protocol versioning and mismatched client/server?
**Recommended:** `Hello{proto_version, build_id}`; server refuses lower major versions with a readable error the client shows as a banner with a "Restart server" action (which kills running processes — the banner says so). Both binaries ship together so this is rare.
**Adopted.**

### Q32. Repository layout: monorepo?
**Recommended:** Monorepo. `crates/` (Rust workspace: `st-proto`, `st-core`, `st-server`, `st-client-core`, `st-native`), `packages/` (Bun workspaces: `app`, `protocol-ts`), `vendor/gpuix` (submodule), `docs/`. One `justfile` for tasks. Bun workspaces for TS, cargo workspace for Rust.
**Adopted.**

### Q33. Testing strategy?
**Recommended:** (1) Rust unit tests for protocol encode/decode round‑trips, replica delta application (property‑based with `proptest`: applying deltas == server snapshot); (2) server integration tests driving a real PTY (`bash -c 'printf …'`) and asserting Snapshot content; (3) gpuix `TestGpuixRenderer` for React chrome; (4) a headless "vt‑conformance" run using `vttest`‑style fixtures from alacritty against our replica; (5) perf harness (`cat` big file, `yes | head`, `btop`) recording frame times.
**Adopted.**

### Q34. Configuration?
**Recommended:** `~/.config/superterminal/config.toml`: font family/size, shell, theme palette, window background mode, keybindings overrides. Read by both server (shell, env) and client (font, theme). Hot‑reload is deferred.
**Adopted.**

### Q35. Packaging for v1?
**Recommended:** `bun build --compile --asset native/*.node --outfile superterminal` producing a single client executable; `superterminald` is a plain cargo release binary placed next to it. macOS `.app` bundling and signing deferred to M6.
**Adopted.**

### Q36. What could kill the project, and what do we do about it early?
**Recommended risks, in order:** (a) GPUI/Zed vendored build weight and API churn → pin the Zed commit gpuix pins; never track Zed main. (b) gpuix hook rejected upstream → we keep the patch; it's ~30 lines. (c) Text shaping performance for full‑screen redraws → run‑level shaping cache, measured in M2 before building anything else. (d) Bun Node‑API edge cases with `ThreadsafeFunction` → smoke‑test `@gpuix/react` counter example under Bun 1.4.0 on day 1 (M0). (e) WSL2 GPU (Vulkan via dozen/D3D12) instability → allow running the client on the Windows side against a WSL server later; for now WSLg Vulkan is a documented requirement.
**Adopted.** M0 exists specifically to de‑risk (a), (d), (e).

---

## F. Addendum — cross‑document questions resolved after the first drafts

The seven detail documents were written in parallel against sections A–E and each ended with "Open questions". The ones below are *conflicts between documents* or *gaps that block implementation*; they are decided here so the detail docs can cite a Qn. Items that are pure M0 verifications (napi symbol retention, `dlopen` from `/$bunfs/`, `AsyncApp` `!Send`, `Bun.spawn detached`, gpuix custom‑element lifetime, shortcut bubbling from a CustomElement) are **not** decisions and are tracked in `HANDOVER.md` §5.

### Q37. How does the Server tell a DATA connection from a CONTROL one? (03‑OQ2 vs 02 §1)
**Adopted:** DATA connections open with the 4‑byte magic `0xFF 'S' 'T' 'D'` before `Hello`; CONTROL connections' first byte is `{`. `02-protocol.md` §1 already specifies this; 03's sniffing text is superseded.

### Q38. Gap detection under coalescing; standalone `ModeChanged`/`TitleChanged`. (02‑OQ1, 02‑OQ2)
**Adopted:** every `Delta` carries `since_seq` in addition to `seq`; the client treats `since_seq != last_seq` as a gap and requests a Snapshot. Standalone `ModeChanged` and `TitleChanged` are **removed**; modes and title are Delta fields (Q16), so such changes are an otherwise‑empty Delta. `Bell` and `SurfaceExited` stay standalone (events, not state).

### Q39. History length signal. (03‑OQ4, 02 §8)
**Adopted:** `scrollback_appended: u32` (Q16) is **replaced** by absolute `history_len: u64` (in `AbsLine` units) on every Delta and Snapshot, plus `history_base`. The client derives appended/evicted counts; alt‑screen transitions and ring eviction are expressible.

### Q40. Resize with multiple clients; reflow; selection across resize. (02‑OQ4, 03‑OQ3, 04‑OQ7)
**Adopted:** last `Resize` wins in v1 (a non‑matching Replica is letterboxed by the element until its next Delta). History **reflow is disabled** in v1 so absolute line ids are never renumbered. The Server **clears the selection on resize** and broadcasts the cleared View State.

### Q41. Row encoding extras. (02‑OQ5, 02‑OQ6, 04‑OQ6)
**Adopted:** rows are sent with trailing default‑style blank cells trimmed (`cols` is known), and each row carries a 1‑byte `wrapped` flag (soft‑wrap continuation) for correct copy/paste. Both apply to Snapshot, Delta and History rows.

### Q42. Idle exit vs. the always‑re‑seeded last Tab. (03‑OQ1)
**Adopted:** a Surface is *pristine* if it is the auto‑seeded shell, has never received `Input`, and has no child processes. Idle shutdown (Q30) counts pristine Surfaces as zero. Term → CONTEXT.md.

### Q43. Which plane carries View State edits (selection, scroll offset)? (07‑OQ2; Q17 vs Q24)
**Adopted:** the **Data Plane**. Both values are produced by the Rust element on the Replica; routing them through JS would add a napi hop for nothing. The Server stores them in the Surface's View State and echoes them on the Control Plane as part of `ev.workspace`, so Q17 ("UI state lives in the Server") still holds. Control‑plane `view.set` remains for tooling/tests.

### Q44. Hidden Tabs: keep mounted, attached, or detached? (05 §4, 07‑OQ1, 05‑OQ4)
**Adopted:** `Attach` gains `mode: Active | Passive`. The visible Tab's Surface is *Active* (rows + everything). Every other Tab **in the active Session** is *Passive*: title/exited/bell/`history_len` only, no rows. Tabs in other Sessions are Detached (their badges come from `ev.workspace`). The React `<SurfaceHost>` mounts **only the visible** `<terminal-grid>`; `st-client-core` keeps an LRU of 4 warm Replicas so re‑activation applies a Snapshot into an existing allocation. 05's "keep N=4 mounted with display:none" is superseded.

### Q45. Style table cap. (02‑OQ10, 01‑OQ7)
**Adopted:** fixed cap of **4 096** entries per Surface; on overflow the Server resets the table and forces a Snapshot to every attached client. No compaction algorithm. `style_idx` stays `u16` on the wire (room to raise the cap as a minor change).

### Q46. Where do the `st` CLI and config parsing live? (07‑OQ3; extends Q32)
**Adopted:** two more crates: `st-config` (TOML schema + loader, shared by Server and CLI; the Bun app parses the same file with `Bun.TOML.parse` and a zod schema — both validated by a shared fixture set) and `st-cli` (binary `st`: `status`, `ls`, `probe`, `kill-server`, `dump-data`). Neither depends on GPUI.

### Q47. Render tests and perf numbers on Linux. (06‑OQ3, 07‑OQ4, 07‑OQ6)
**Adopted:** gpuix's `TestGpuixRenderer` and e2e run on **macOS CI**; Linux headless (Xvfb + lavapipe) is a non‑blocking experiment. The M2 perf gate is decided by **macOS numbers**; WSL2 numbers are recorded and must be within **2×**. Nightly perf runs on a **self‑hosted runner** (the dev box), never on shared runners. Q3 is unchanged: Linux remains the primary *development* platform for everything that is not a GPU test.

### Q48. Small defaults the docs asked for.
**Adopted:** initial Session name `Default`. `Session.active_tab` is persisted and `tab.set_active` exists on the Control Plane. `SurfaceStatus` includes `cwd` and `has_foreground_child` (Server samples the PTY's foreground process group). OSC 52 clipboard is **off**. The Server answers OSC 10/11 colour queries from `[theme]` via `st-config` (it still never renders). `alacritty_terminal` is taken from **crates.io** (0.26.x), not Zed's fork. Cursor is **hidden** while scrolled up. Linux **primary selection** is written on select (middle‑click paste works); `bold_is_bright` is a config flag, default `false`. `Input` to an Exited Surface is a per‑message `DataError`, not connection‑fatal. `CreateSurface` may carry an env allow‑list from the client (`PATH`, `LANG`, `LC_*`, `SSH_AUTH_SOCK`, `DISPLAY`, `WAYLAND_DISPLAY`) so a long‑lived daemon does not hand out a stale environment.

### Q49. Wire message for View State edits (implementation gap found in `st-client-core`)
Q43 routes selection and scroll offset over the Data Plane, but the first `st-proto` implementation had no client→server message for them. **Adopted:** `SetViewState { surface, scroll_offset: Option<AbsLine>, selection: Option<Selection> }`, data-plane msg type **`0x0016`**. The Server stores it on the Surface and echoes it to control-plane subscribers in `ev.workspace`; control-plane `view.set` remains for tooling and tests. Implemented in `crates/st-proto/src/data.rs`.

### Q50. `MAX_FRAME` size
`02-protocol.md` §1 says 8 MiB; `06-testing-perf-ci.md` proposed 16 MiB; the implementation uses **16 MiB**. **Adopted: 16 MiB.** A 1 000-row History page at 200 columns is ~1.6 MB, and Snapshots of very large windows plus a style table must fit with headroom. `02-protocol.md`'s 8 MiB figure is superseded.

### Q51. Socket filename
Three names appeared across the plan (`control.sock` in 05/07, `server.sock` in 02, `sock` in 03). **Adopted: `server.sock`.** Q37 froze a single socket carrying both planes (distinguished by first-byte sniffing), so a "control" name is misleading, and `st-config` — the shared schema used by the Server and the `st` CLI — already resolves `server.sock`. The client still probes the two legacy names so a daemon built from an older doc is found rather than duplicated.

### Q52. GPUI renders through `wgpu` here, not blade (M0 finding)
The vendored Zed checkout that gpuix pins renders via `wgpu`. On the WSL2 dev box the NVIDIA Vulkan ICD is registered but broken (`libGLX_nvidia.so.0` does not export `vk_icdGetInstanceProcAddr`), so wgpu falls back to the **GL** backend over the WSLg D3D12 adapter. **Adopted:** this is acceptable for development, `LIBGL_ALWAYS_SOFTWARE` is the escape hatch on this host rather than a Vulkan ICD override, and — reinforcing Q47 — **no frame time measured on this host may decide the M2 gate**. GL-on-D3D12-on-RDP is not representative.

### Q53. A second gpuix patch is required for Linux input testing (M0 finding)
`UiCommand::DispatchMouse` dispatches inside `WindowHandle::<GpuixView>::update`, which leases the view while gpuix's own root MouseUp handler updates it — so every synthetic left click panics the GPUI thread and all subsequent napi calls fail. Upstream misses it because `TestGpuixRenderer` is macOS/Windows-only. **Adopted:** carry `patches/0002-linux-simulate-mouse-double-lease.patch` (7 lines, mirroring the `DispatchKey` arm's use of `AnyWindowHandle`) alongside the factory hook, and open upstream PRs for both. Without it no automated input test can run on Linux, which M3's testing story depends on.

## Shared understanding reached
The decisions above (Q1–Q53) are frozen for planning. Downstream documents: `01-architecture.md`, `02-protocol.md`, `03-server.md`, `04-client-native.md`, `05-client-app.md`, `06-testing-perf.md`, `07-milestones.md`, `CONTEXT.md`, `docs/adr/*`, `HANDOVER.md`.
