# Windows client + WSL server

The supported Windows setup keeps the daemon (and every PTY) inside WSL2 and
runs the GUI natively on Windows. A socket file cannot cross the VM boundary,
so the two sides talk loopback TCP (`superterminald --tcp`, `tcp://` targets).
WSL's localhost relay makes `127.0.0.1` work both ways on default NAT
networking; no `.wslconfig` change is needed. (If it ever stops working,
`networkingMode=mirrored` under `[wsl2]` in `%USERPROFILE%\.wslconfig`
followed by `wsl --shutdown` is the fallback.)

## Prerequisites (Windows)

- Windows 11, WSL2 with any distro as the home side.
- Rust stable MSVC (`rustup update stable`) plus the pinned toolchain from
  `vendor/gpuix/rust-toolchain.toml` (cargo fetches it automatically).
- Visual Studio 2022 Build Tools with the **Desktop development with C++**
  workload (`link.exe` must exist under
  `C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC`).
- Bun 1.4.x for Windows and Git for Windows.

## Build

In PowerShell (or `cmd.exe`), with this repo checked out at e.g.
`C:\Users\<you>\superterminal`:

```powershell
git submodule update --init
git -C vendor\gpuix submodule update --init --depth 1 --recursive zed
# apply every patch, skipping ones already applied:
#   git -C vendor\gpuix apply --check --reverse ..\..\patches\0001-factory-hook.patch
#   (and 0002), else git -C vendor\gpuix apply ..\..\patches\<name>.patch
cd crates\st-native
cargo build
mkdir dist
copy target\debug\st_native.dll dist\superterminal-native.win32-x64-msvc.node
cd ..\..
bun install
```

## Run

1. In WSL, start (or reuse) a daemon with a TCP listener:
   ```bash
   superterminald --tcp 127.0.0.1:7171
   ```
   Only loopback addresses are accepted; TCP peers carry no uid credential,
   so a non-loopback `--tcp` is refused at start-up.
2. On Windows:
   ```bat
   set SUPERTERMINAL_TCP=127.0.0.1:7171
   set NAPI_RS_NATIVE_LIBRARY_PATH=C:\Users\<you>\superterminal\crates\st-native\dist\superterminal-native.win32-x64-msvc.node
   bun packages\app\src\app.tsx
   ```
   `SUPERTERMINAL_TCP` turns every socket path in the app into
   `tcp://127.0.0.1:7171` (control plane, data plane pre-warm and
   `<terminal-grid>`); the daemon is never spawned from Windows. `--tcp
   127.0.0.1:7171` on the app command line is equivalent.

## Notes and limits

- The data plane reconnects and re-attaches like the Unix transport; the
  framing is transport-agnostic (`02-protocol.md` §1.1).
- `st --tcp 127.0.0.1:7171 status|ls|probe` works from either side for
  headless checks.
- Primary-selection (middle-click paste) is a no-op on Windows: GPUI only
  exposes `write_to_primary` on Linux. Explicit copy/paste is unchanged.
- PTYs live in WSL, so ConPTY never enters the picture; the Q3 Windows
  caveats in `docs/plan/00-grilling.md` still apply to a hypothetical
  all-Windows server.
