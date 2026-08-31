# HANDOVER — implementing superterminal

Audience: an AI coding agent (or a human) starting implementation from this planning‑only repository. Everything below is what you need to work productively without the planning conversation. Planning finished **2026‑08‑31**; no code exists yet.

---

## 1. What you are building (30 seconds)

A terminal whose terminals live in a persistent per‑user **Server** (`superterminald`, Rust) and are displayed by a thin **Client** (Bun 1.4.0 running React on **gpuix**, i.e. Zed's GPUI, with a Rust‑native `<terminal-grid>` element). Quit the window: processes keep running. Relaunch: the grid is back in well under a second. Sessions, tabs, a command palette, a native scrollbar, no prefix key, no modes. It is the slice shown in Mitchell Hashimoto's Superlogical demo (2026‑08‑28), built on our own stack.

Reference architecture terms — always use the glossary in [`CONTEXT.md`](./CONTEXT.md): *Server, Client, Workspace, Session, Tab, Surface, Replica, Attach (Active/Passive), Snapshot, Delta, History, View State, Control Plane, Data Plane, Command, Exited, Pristine.*

## 2. Read in this order (≈ 45 min)

1. [`docs/plan/00-grilling.md`](./docs/plan/00-grilling.md) — the 48 decisions (Q1–Q48) with reasons. **This is the constitution.** If a detail doc disagrees with it, the grilling doc wins; if you must change a decision, edit *this file first* (add a Qn, mark the old one superseded) and then the detail docs.
2. [`CONTEXT.md`](./CONTEXT.md) — glossary. Use these nouns in code, commits, docs.
3. [`docs/adr/0001…0007`](./docs/adr/) — the seven hard‑to‑reverse choices and why.
4. [`docs/plan/01-architecture.md`](./docs/plan/01-architecture.md) — processes, threads, crate graph, failure modes.
5. [`docs/plan/02-protocol.md`](./docs/plan/02-protocol.md) — wire spec (Control Plane JSON, Data Plane binary), with Rust/TS types.
6. Then the doc for your lane: [`03-server.md`](./docs/plan/03-server.md), [`04-client-native.md`](./docs/plan/04-client-native.md), [`05-client-app.md`](./docs/plan/05-client-app.md).
7. [`06-testing-perf-ci.md`](./docs/plan/06-testing-perf-ci.md) — what "done" means per layer.
8. [`docs/plan/07-milestones.md`](./docs/plan/07-milestones.md) — **the task list you execute** (87 tasks, ids `M0-01 … M6-10`, each ≤ 1 day with an acceptance test).

## 3. Non‑negotiable invariants

These are restated from the ADRs because breaking any one of them silently invalidates the rest of the plan.

| # | Invariant | Source |
|---|---|---|
| I1 | The Server owns every PTY and terminal state machine. The Client never parses VT bytes. | ADR‑0002, ADR‑0003, Q6–Q7 |
| I2 | Cell data never passes through JavaScript. Deltas go Server → Rust native module → GPUI paint. React carries chrome and a `surfaceId`. | ADR‑0005, Q11, Q13 |
| I3 | Two connections per Client: Control Plane (Bun, NDJSON) and Data Plane (Rust, `0xFF"STD"` magic + `u32 len | u16 type | postcard`). | ADR‑0007, Q14–Q15, Q37 |
| I4 | No prefix key, no modal layer, no status bar. Multiplexer features are native GUI affordances only. | ADR‑0001, Q4 |
| I5 | gpuix is vendored at `vendor/gpuix` pinned to **0.6.0**, GPUI pinned to the Zed commit gpuix pins. Never track Zed `main`. The patch lives in `patches/0001-factory-hook.patch` and must stay ≤ ~40 lines. | ADR‑0006, Q12, Q36a |
| I6 | `alacritty_terminal` (crates.io 0.26.x) is confined to `crates/st-core/src/vt/alacritty.rs` behind the `VtEngine` trait. Its types never appear on the wire. | ADR‑0004, Q8, Q48 |
| I7 | UI state that must survive a Client relaunch (tabs, sessions, active tab, scroll offset, selection) lives in the Server. Client‑only state: window geometry, focus, palette text. | Q17, Q43 |
| I8 | `st-proto` depends only on `serde` + `postcard` (+ `ts-rs` dev‑only). Any wire change after M1‑02 needs a protocol version bump and one owner. | Q32, 07 §Parallelization |
| I9 | `crates/st-client-core` has **no GPUI dependency** and is fully unit‑testable without a GPU. | 04 §11 |
| I10 | Every `<text>` in React sets `color` (GPUI does not inherit). | gpuix README |

## 4. Repository layout you will create (M0‑01)

```
superterminal/
├── Cargo.toml                 # workspace; rust-toolchain.toml = 1.96
├── package.json               # bun workspaces: packages/*
├── bunfig.toml                # [run]/[test] preload → sets NAPI_RS_NATIVE_LIBRARY_PATH
├── justfile                   # build-native, dev, server, test, fmt, lint, vendor-patch, clean-vendor
├── crates/
│   ├── st-proto/              # wire types (Control + Data), postcard, ts-rs export
│   ├── st-config/             # config.toml schema + loader (Server, CLI)            [Q46]
│   ├── st-core/               # VtEngine trait, alacritty adapter, PTY, Publisher/Delta production
│   ├── st-server/             # superterminald: sockets, Workspace actor, persistence, idle exit
│   ├── st-client-core/        # Replica, key/mouse encoders, Data Plane client (no GPUI)
│   ├── st-native/             # napi cdylib: re-exports gpuix-native + <terminal-grid> CustomElement
│   └── st-cli/                # `st` binary: status, ls, probe, kill-server, dump-data   [Q46]
├── packages/
│   ├── app/                   # Bun + React chrome (@gpuix/react), control-plane client, packaging
│   ├── native/                # loader shim + built .node artifacts (gitignored)
│   └── protocol-ts/           # generated TS types from st-proto (ts-rs), CI diff-checked
├── vendor/gpuix/              # git submodule @ 0.6.0 (itself vendors Zed for GPUI)
├── patches/0001-factory-hook.patch
├── docs/{plan,adr,PINS.md,DEV.md,perf/}
└── CONTEXT.md  HANDOVER.md  README.md
```

## 5. M0 verifications — do these before believing the plan

The plan is grounded in verified facts (see §9), but seven things could only be settled by running code. Each is a task in M0 (07‑milestones) — record the answer in `docs/DEV.md` and, if it changes a decision, add a Qn to `00-grilling.md`.

| # | Question | If "no" |
|---|---|---|
| V1 | ✅ **VERIFIED on Linux/WSLg** (2026‑08‑31): 50 synthetic clicks drive the gpuix counter 0→50 in 1.54 s under Bun 1.4.0, no panic or hang; `<hello-box>` from our own `st-native` crate lays out and re‑shapes correctly. macOS still unverified (no host available). |
| V2 | Do `#[napi]` registration symbols from `gpuix-native` (rlib) survive linking into our `st-native` cdylib on macOS (`-dead_strip`)? (04 §1.3, M0‑07) | Use the thin delegating `GpuixRenderer` wrapper described in 04 §1.3. |
| V3 | ✅ **RESOLVED**: register factory *constructors*, not boxed instances — `with_defaults()` runs once per window and once per `TestGpuixRenderer`, so a drained list leaves later registries empty. Patch 0001 reflects this. |
| V4 | Can we obtain an `AsyncApp`/foreground executor handle at init to wake GPUI from the Data Plane thread (`cx.notify()` path)? (01 §5, 04 §5) | Extend the patch with a run‑loop ping; or fall back to gpuix `tick()` on macOS only. |
| V5 | Do key events a `CustomElement` declines propagate to a React ancestor's `onKeyDown`? (Q23, 07‑OQ8) | Emit a napi `shortcut` event from the element with the key chord; React dispatches. |
| V6 | Does `process.dlopen` accept a `.node` under `/$bunfs/` in a `bun build --compile --asset` binary? (05 §10) | Keep the copy‑to‑`$XDG_CACHE_HOME/superterminal/<build_id>/` step. |
| V7 | Does `Bun.spawn` support `detached: true` (new process group) in 1.4.0? (Q30, 05‑OQ2) | Server double‑forks itself on `--daemonize`. |

## 6. How to work on this repo (agent protocol)

1. **One task per session.** Pick the lowest‑numbered unblocked task in `07-milestones.md` for your lane (Server / Native / Chrome — see 07 §Parallelization). Check its `Deps` column.
2. **Read the acceptance test first**, then the referenced section of the detail doc, then write code. If the doc is ambiguous, look for a Qn; if none exists, decide, and append `Qn` to `00-grilling.md` §F with one paragraph of reasoning.
3. **Finish = acceptance test passes + `just test` green on Linux.** macOS is verified at milestone close (07 rule 2). Say plainly if you could not run something.
4. **Never edit `st-proto` after M1‑02 without a version bump** (07 rule 1). Other lanes rebase.
5. **Docs are code.** When you implement something differently from the doc, update the doc in the same change. Keep `CONTEXT.md` a glossary only — no implementation detail.
6. **Vendored code:** never edit `vendor/gpuix` in place. Edit the patch file, `just vendor-patch`, and keep the upstream PR link in `docs/PINS.md`.
7. **Perf numbers** are recorded only on a quiet machine, to `docs/perf/<scenario>-<host>-<date>.json` (06 §3). Never trust numbers from CI runners.
8. **Commit style:** `M2-07: shape runs per row with RunCache` — task id first. Reference Qn/ADR in the body when a decision was involved.
9. **Do not** add dependencies casually: `cargo deny` runs in CI; Apache‑2.0/MIT/BSD are fine, MPL‑2.0 is decided after the first `cargo deny` in M0 (06‑OQ8).

### Session start prompt (copy/paste)

> You are implementing superterminal. Read `HANDOVER.md`, then `docs/plan/00-grilling.md`, `CONTEXT.md`, and the section of the detail doc named in task **`<Mx-yy>`** of `docs/plan/07-milestones.md`. Implement only that task. Its acceptance test must pass and `just test` must be green. Update docs you deviate from. Report what you verified, what you could not run, and any new Qn you added.

## 7. Milestone map (from 07)

| Milestone | Goal | Exit signal | Effort |
|---|---|---|---|
| **M0** De‑risk & skeleton | Toolchain end‑to‑end; `<hello-box>` painted from *our* crate on Linux + macOS; patch ≤ 40 lines; CI warm < 15 min | `just dev` shows hello‑box on both OSes | 6 d |
| **M1** Protocol + server core | `st-proto` frozen; Server runs a PTY through alacritty, emits Snapshot/Delta; `st probe` prints a live grid | `st probe` shows `ls` output from a real shell | 5 d |
| **M2** Native grid gate | `<terminal-grid>` paints Replica from live Deltas; run‑shaping cache; **go/no‑go**: `cat` 100 MB at p95 < 16.6 ms (macOS decides, WSL2 ≤ 2×) | M2‑10 passes or M2‑11 fallback spike chosen | 6 d |
| **M3** Input & interaction | Keys, mouse, selection, clipboard, scrollbar/history paging, resize; nvim/htop/btop usable; vttest checklist | Demo steps 2, 4, 7, 9 work | ~5 d |
| **M4** Workspace + chrome | Control Plane, WorkspaceStore, TabStrip, Sessions, palette, banners, reconnect, `workspace.json` | **Demo script (07 Appendix A) passes end‑to‑end** | ~7 d |
| **M5** Polish | Blurred window (macOS), Linux fallbacks, config TOML, vertical tabs, exited UX, bell, cwd inheritance, `st status` | Demo steps 1, 10, 11 polished | 4 d |
| **M6** Packaging | `bun build --compile --asset`, server placement, unsigned `.app` skeleton, nightly perf on self‑hosted runner | Single binary launches on a clean machine | 3 d |

Total ≈ 53 engineer‑days serial; 33–40 calendar days with three lanes. **M2‑10 is the only gate that can kill the rendering approach; do not build M3/M4 native work before it passes.**

## 8. Open questions still open (by owner)

Everything that was a *conflict* has been decided in `00-grilling.md` §F (Q37–Q48). What remains are honest unknowns, each owned by one doc and answered during the milestone shown:

| Owner | Question | When |
|---|---|---|
| 02 | Does alacritty report per‑line or full‑screen damage after a scroll? (drives Delta size for `ls`; Server may need to diff line‑id rings) | M1‑06 |
| 02 | Reserve an auth token field in `Hello` for the future SSH transport? | M1‑02 (cheap: reserve now) |
| 03 | Evicted‑lines counting via `Handler` shim — does the proptest show drift? | M1‑09 |
| 03 | Ack window = 4, slow‑client snapshot after 3 s, disconnect at 30 s — tune with the perf harness | M2‑09 |
| 04 | `stats` via `get_prop` vs `tracing` file for frame timing (must be fixed before M2 so numbers compare) | M2‑01 |
| 04 | Config key names: Option‑as‑Alt, Backspace DEL/BS, `alt_screen_scroll`, `word_chars` | M5‑03 |
| 05 | Hot‑reload guard so `bun --hot` never calls `render()` twice | M0‑09 |
| 06 | Which gpuix test‑renderer API names exist at the pinned commit (`createTestRoot`, `drainNativeEvents`, `getPaintedText`; `getCustomProp` undocumented) | M0‑04 |
| 06 | Is `gpui::TestAppContext` usable through the vendored path dep with `test-support`? | M2‑02 |
| 06 | Perf regression policy: relative (−15 % vs 7‑day median) on CI, absolute Q27 numbers on dev boxes | M5 close |

## 9. Facts the plan relies on (verified 2026‑08‑31)

- **gpuix 0.6.0** (`remorses/gpuix`, Apache‑2.0): React → `applyBatch(json)` → Rust `RetainedTree` → GPUI. `CustomElement`/`CustomElementFactory` traits; `CustomElementRegistry::register()` is Rust‑only and invoked via `with_defaults()` inside `GpuixRenderer::init` — no plugin loading yet ("phase 2 planned"). `canvas` element is planned, not implemented. `crate-type = ["cdylib","rlib"]`, napi‑rs v3, GPUI as path dep `../../zed/crates/gpui` (vendored Zed). Prebuilt `.node`: darwin‑arm64, linux‑x64‑gnu, win32‑x64‑msvc. Loader honours `NAPI_RS_NATIVE_LIBRARY_PATH` first. On macOS JS drives `tick()`; on Linux/Windows GPUI's loop runs on its own thread. `TestGpuixRenderer` is GPU‑backed; Linux "not yet". Benchmark: ~592 B retained/element, 30 ms to apply 220 k ops → the grid must be native (ADR‑0005).
- **Bun 1.4.0** (2026‑08‑20): Node‑API supported; `Bun.Terminal` PTY exists (unused by us — Server owns PTYs); `bun build --compile --asset` embeds files under `/$bunfs/`, enumerable via `Bun.embeddedFiles`; `Bun.TOML.parse`; `bun --hot`; `bun test`; `Bun.connect({ unix })`; `package.json` `overrides` accept `npm:`/`catalog:` only.
- **alacritty_terminal 0.26** (Apache‑2.0, MSRV 1.85): `Term::new`, `damage()`/`reset_damage()`, `TermDamage::{Full, Partial}`, `EventListener`, `vte::ansi::Processor::advance`. Its `Handler` never sees OSC 7 → separate `vte::Perform` sniffer for cwd.
- **portable-pty 0.9**: `openpty`, `PtySize`, `CommandBuilder`, `try_clone_reader`/`take_writer`, `Child::wait`.
- **Zed `terminal_view`** (`crates/terminal_view/src/terminal_element.rs`) is the reference for painting an alacritty grid in GPUI: advance‑of‑`m` metrics, batched text runs, paint order bg → glyphs → decorations → cursor, `InputHandler` for IME, dim ×0.7, inverse swap.
- **Local toolchain:** bun 1.4.0, cargo 1.96.0, node present; dev box is WSL2 (Linux 6.18). macOS arm64 is the second required platform.
- **M0 build reality (measured 2026‑08‑31):** `gpuix-native` cold debug 3 min 1 s; `st-native` cold debug 2 min 59 s, cold **release 12 min 2 s**. gpuix has no `v0.6.0` git tag — the release is tagged per npm package; pinned commit `dfb83f3e096b0d3130e7b63660d9b6810cb5855c`, its Zed submodule (`remorses/zed`, branch `gpuix`) at `8b94defe56992b3ca4ffd4853ace741d8168111a`. The build needs `-dev` system packages — see the apt one-liner in `docs/DEV.md` §1 (`cmake`/`clang` turn out NOT to be needed). Linux introspection is limited to `getAutomationTree()`; `getPaintedText()` returns `[]` and `captureScreenshot()` refuses.

## 10. Things that look like bugs but are decisions

- Closing a Tab kills its Surface; quitting the Client does not (Q21).
- An Exited Surface stays on screen until Enter/click (Q22).
- No copy‑on‑select on macOS (explicit Copy only); Linux writes the primary selection (Q24, Q48).
- History reflow on resize is disabled (Q40).
- Style table resets (and forces a Snapshot) at 4 096 styles (Q45).
- Hidden Tabs are Passive attaches with no rows; only the visible `<terminal-grid>` is mounted (Q44).
- Windows builds may fail without blocking a milestone (Q3).
- The Server knows the theme only to answer OSC 10/11 queries; it never renders (Q48).

## 11. Out of scope for v1 — do not build

Remote hosts over SSH, split panes, web client, Windows support, theme UI, ligatures, Sixel/Kitty graphics, scrollback search, AI features, plugins, hot‑reloading config, macOS signing/notarization (Q5, Q34, Q35). Each has a documented hook in `01-architecture.md` §7.

## 12. Provenance

Planning was produced in one session on 2026‑08‑31 from: the Superlogical demo video and post, gpuix 0.6.0 source and design docs, Bun 1.4.0 release notes and docs, alacritty_terminal/portable-pty docs, and Zed's terminal element. Seven detail documents were written in parallel against the frozen Q1–Q36 and then reconciled (Q37–Q48). Where a fact could be looked up it was; where a decision was required it was made and its reasoning recorded. Treat anything not cited to a Qn, ADR, or §9 fact as a proposal you may change.
