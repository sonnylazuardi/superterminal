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
   Start it with a **clean environment**: surfaces inherit the daemon's
   environment, so exported `XDG_*` overrides leak into every shell. To
   isolate a trial daemon, use the flags — never the env:
   ```bash
   superterminald --socket /tmp/st-win/server.sock \
     --state-dir /tmp/st-win/state --tcp 127.0.0.1:7171 --no-idle-exit
   ```
   (A daemon started under `XDG_STATE_HOME=/tmp/...` once broke `opencode2`
   service discovery in all its shells: the client looked for its service
   registration under the scratch dir, found nothing, and timed out starting
   a duplicate service.)
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

## Packaged builds (exe + MSI)

`packaging/windows/` holds a WiX 3.11 manifest (`Product.wxs`, per-user, no
admin). The full chain, all on Windows:

1. Release the native module (debug CRT is not redistributable, so this
   must be a release build):
   ```bat
   cd crates\st-native
   cargo build --release
   mkdir ..\..\packages\native
   copy target\release\st_native.dll ..\..\packages\native\superterminal-native.win32-x64-msvc.node
   ```
2. Compile the single-file client (run from the repo root):
   ```bat
   bun build --compile packages/app/src/app.tsx --outfile dist/superterminal.exe
   copy packages\native\superterminal-native.win32-x64-msvc.node dist\
   ```
   The `.node` ships **side by side**, found via the "beside a compiled
   binary" probe in `packages/app/src/native/locate.ts`. Do not use
   `--asset` to embed it: on Bun 1.4.0-canary an asset-embedding compile
   produces an exe whose entry never evaluates (verified; the plain compile
   runs fine).
   Flip the exe to the GUI subsystem or Windows parks a console next to
   every window (needs `editbin.exe` from the MSVC install):
   ```bat
   "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\<ver>\bin\Hostx64\x64\editbin.exe" /SUBSYSTEM:WINDOWS dist\superterminal.exe
   ```
3. Build and install the MSI (WiX 3.11 `candle`/`light` on `PATH`; per-user,
   installs to `%LOCALAPPDATA%\Superterminal`, sets user
   `SUPERTERMINAL_TCP=127.0.0.1:7171`, adds a Start Menu shortcut):
   ```bat
   cd packaging\build  &  rem a scratch dir with superterminal.exe,
                         rem superterminal-native.win32-x64-msvc.node, Product.wxs
   candle.exe Product.wxs -o obj\
   light.exe obj\Product.wixobj -o Superterminal-0.1.0.msi
   msiexec /i Superterminal-0.1.0.msi /passive
   ```
   Unsigned, like the rest of v1 packaging (Q5/Q35): expect a SmartScreen
   prompt on first install from another machine.

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
