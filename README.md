# superterminal

A GPU‑rendered, native multiplexer terminal for Windows, Linux and Mac. Rust server (`superterminald`) owns the terminals; a Bun 1.4.0 + React client renders them through [gpuix](https://github.com/remorses/gpuix) (React bindings for Zed's GPUI) with a native Rust `<terminal-grid>` element.

**Status: implemented and running.** The daemon owns live PTYs and streams
Snapshots/Deltas; the client renders them in a native window with a vertical
tab sidebar, command palette, and reconnect. Run it via [`docs/DEV.md`](./docs/DEV.md)
(Linux/WSL) or [`docs/WINDOWS.md`](./docs/WINDOWS.md) (native Windows client
against a WSL server, exe + MSI included). Planning history is preserved below.

## What works

- **Server** (`superterminald`): per-Surface PTYs over `portable-pty`,
  `alacritty_terminal` VT engine behind the `VtEngine` trait, Snapshot/Delta
  fan-out with ack window and slow-client Snapshot, Workspace actor
  (Sessions → Tabs → Surfaces), `workspace.json` persistence with cwd
  tracking, idle exit, `st` CLI (`status`, `ls`, `probe`, `kill-server`,
  `dump-data`, `config`).
- **Protocol** (`st-proto` v1.0): Control Plane (NDJSON) + Data Plane
  (`u32 len | u16 type | postcard`) on one sniffed socket, plus **loopback
  TCP** (`--tcp`, `tcp://`) for the Windows/WSL split.
- **Client**: native `<terminal-grid>` (run-shaping cache, selection,
  mouse reporting, scrollbar + lazy history, resize, IME/focus handling),
  React chrome (sidebar/strip toggle, palette, toasts, banners, keybindings),
  server auto-spawn and reconnect.
- **Platforms**: Linux/WSLg and native Windows (MSVC build, Direct3D,
  per-user MSI, no-console exe) live against a WSL daemon; macOS builds are
  configured but not yet exercised on hardware.

## Run and build

### Linux (incl. WSL2 — the primary dev setup)

```bash
# prerequisites: rustup (stable + 1.97.1), Bun 1.4.0, GPU/dev libraries —
# full list in docs/DEV.md §1
git clone <repo> && cd superterminal
git submodule update --init
git -C vendor/gpuix submodule update --init --depth 1 --recursive zed
for p in patches/*.patch; do git -C vendor/gpuix apply "../../$p"; done
./scripts/run.sh              # build what is missing, start daemon + client
./scripts/run.sh --no-build   # run what is already built
```

The window appears via WSLg (X11 backend for real decorations). Headless
checks anytime: `./target/debug/st status`, `st ls`, `st probe <id>`.

### Windows (native client, WSL server)

```powershell
# prerequisites: Rust stable MSVC, VS 2022 Build Tools with C++ workload,
# Bun 1.4.x, Git for Windows — details in docs/WINDOWS.md
git clone <repo>; cd superterminal
git submodule update --init
git -C vendor\gpuix submodule update --init --depth 1 --recursive zed
# apply patches\*.patch inside vendor\gpuix, then:
cd crates\st-native; cargo build; cd ..\..
copy target\debug\st_native.dll dist\superterminal-native.win32-x64-msvc.node
bun install
```

```powershell
# terminal 1 (WSL):  superterminald --tcp 127.0.0.1:7171
# terminal 2 (Windows):
$env:SUPERTERMINAL_TCP = "127.0.0.1:7171"
$env:NAPI_RS_NATIVE_LIBRARY_PATH = "C:\...\superterminal\crates\st-native\dist\superterminal-native.win32-x64-msvc.node"
bun packages\app\src\app.tsx
```

Packaged form: `packaging/windows/` builds `superterminal.exe` +
side-by-side `.node` (`bun build --compile`, then `editbin /SUBSYSTEM:WINDOWS`
for no-console launch) and a per-user MSI (WiX 3.11). Full chain in
[`docs/WINDOWS.md`](./docs/WINDOWS.md).

### macOS (configured, not yet run on hardware)

```bash
xcode-select --install   # Metal / CoreText come with the SDK, nothing else
rustup toolchain install 1.97.1
# then the same clone → submodules → vendor-patch → build flow as Linux:
cargo build --workspace
(cd crates/st-native && cargo build --release)
bun install && bun packages/app/src/app.tsx
```

`cargo check --workspace --all-targets --target aarch64-apple-darwin` passes
from Linux, so the non-GPU crates are cfg-correct; the native GUI build,
traffic-light fit and blurred background still await a first Mac run
(see `docs/DEV.md` "macOS status").

## Roadmap (plan → reality)

From [`docs/plan/07-milestones.md`](./docs/plan/07-milestones.md):

- [x] **M0** De-risk & skeleton — toolchain end to end, vendored gpuix 0.7.0
  with the factory-hook + mouse-lease patches, `<hello-box>`.
- [x] **M1** Protocol + server core — frozen wire types, PTY engine,
  attach/detach fan-out, `st probe` on a live grid, history.
- [x] **M2** Native grid gate — `<terminal-grid>` paints live Deltas; app
  shows a real prompt.
- [x] **M3** Input & interaction — keys, mouse + selection, clipboard,
  wheel/alt-screen, scrollbar paging, cursor shapes, resize.
- [x] **M4** Workspace + chrome — control-plane commands, sessions/tabs,
  palette, banners, reconnect, persistence; vertical sidebar default.
- [~] **M5** Polish — config TOML, themes, exited UX, bell, cwd inheritance,
  `st status`, fonts/emoji/HiDPI fixes all landed; macOS blur/traffic-light
  fit and multi-day dogfooding still open.
- [~] **M6** Packaging — Windows exe + per-user MSI ship and install; macOS
  `.app`, Linux tarball, release CI, nightly perf still open.
- **[+] Beyond the plan** — loopback TCP transport, Windows-client/WSL-server
  split, gpuix 0.7.0 bump, `fixing-gpuix-layout` skill, remembered window
  size / tab layout / sidebar width (Client State, ADR 0008), **split Panes**
  with a right-click tab Menu and draggable dividers (ADR 0009, protocol 1.1).
- **[ ] Out of scope (unchanged)** — remote SSH, web client, ligatures,
  graphics protocols, scrollback search, signing/notarization.

## Documents

| File | Purpose |
|---|---|
| [`HANDOVER.md`](./HANDOVER.md) | Entry point for an AI agent (or human) picking up implementation |
| [`CONTEXT.md`](./CONTEXT.md) | Ubiquitous language / glossary — use these words everywhere |
| [`docs/plan/00-grilling.md`](./docs/plan/00-grilling.md) | The 36 decisions, with reasoning, that everything else depends on |
| [`docs/plan/01-architecture.md`](./docs/plan/01-architecture.md) | Processes, threads, connections, crate layout, failure modes |
| [`docs/plan/02-protocol.md`](./docs/plan/02-protocol.md) | Wire protocol: Control Plane (JSON) and Data Plane (binary) |
| [`docs/plan/03-server.md`](./docs/plan/03-server.md) | `superterminald`: workspace actor, VT engine, PTYs, persistence |
| [`docs/plan/04-client-native.md`](./docs/plan/04-client-native.md) | Rust native module: gpuix patch, Replica, `<terminal-grid>` painting & input |
| [`docs/plan/05-client-app.md`](./docs/plan/05-client-app.md) | Bun/React chrome: tabs, sessions, palette, control‑plane client, packaging |
| [`docs/plan/06-testing-perf-ci.md`](./docs/plan/06-testing-perf-ci.md) | Test pyramid, VT conformance, perf budgets, CI |
| [`docs/plan/07-milestones.md`](./docs/plan/07-milestones.md) | M0–M6 work breakdown with task ids, estimates, acceptance tests |
| [`docs/adr/`](./docs/adr/) | Architecture decision records (the hard‑to‑reverse choices) |
