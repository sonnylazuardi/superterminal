# DEV — building superterminal from a clean checkout

Two build worlds live in this repo and they do not overlap:

| World | Command | Toolchain | Needs a GPU stack |
|---|---|---|---|
| Server / protocol / CLI (`st-proto`, `st-core`, `st-server`, `st-client-core`, `st-cli`, `st-config`) | `cargo build --workspace` | root `rust-toolchain.toml` → `stable` | no |
| Native client (`vendor/gpuix`, `crates/st-native`) | `cd crates/st-native && cargo build` | `vendor/gpuix/rust-toolchain.toml` → **1.97.1** | yes |

`crates/st-native` is in the root `Cargo.toml` `exclude` list and carries its own
`[workspace]` table, so nothing you do in the first column ever compiles GPUI.
Pins, timings and verification status: `docs/PINS.md`. Test/CI policy:
`docs/plan/06-testing-perf-ci.md`.

---

## 1. Prerequisites

### Everywhere

- **rustup**, plus the pinned toolchain: `rustup toolchain install 1.97.1`.
  Both `vendor/gpuix/rust-toolchain.toml` and `vendor/gpuix/zed/rust-toolchain.toml`
  request 1.97.1, and rustup honours the *nearest* file, so anything built under
  `vendor/` or in `crates/st-native` uses it whether you ask for it or not. The
  root workspace stays on `stable`.
- **Bun 1.4.0** (`curl -fsSL https://bun.sh/install | bash`).
- **git ≥ 2.30** — the Zed submodule is fetched by exact SHA at `--depth 1`.

### Linux (Debian/Ubuntu) — the one-line apt command

```
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
  build-essential pkg-config \
  libfontconfig-dev libfreetype-dev libexpat1-dev uuid-dev libpng-dev zlib1g-dev libbrotli-dev \
  libx11-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-shm0-dev \
  libxcb-randr0-dev libxcb-xfixes0-dev libxcb-xkb-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libvulkan-dev libvulkan1 mesa-vulkan-drivers libasound2-dev libssl-dev
```

That list is what an actual cold build on this repo needed, not a copy of Zed's
docs. What each group is for:

| Group | Why |
|---|---|
| `libfontconfig-dev`, `libfreetype-dev` (+ `libexpat1-dev`, `uuid-dev`, `libpng-dev`, `zlib1g-dev`, `libbrotli-dev`) | `yeslogic-fontconfig-sys` and `freetype-sys`, pulled in by `font-kit` via `gpui`. **This is the first thing that fails** on a bare box, ~45 s in. |
| `libx11-dev`, `libxcb*-dev` | `gpui_platform`'s `x11` feature. |
| `libxkbcommon-dev`, `libxkbcommon-x11-dev` | keymaps on both X11 and Wayland. |
| `libwayland-dev` | `gpui_platform`'s `wayland` feature; also supplies the `wayland-scanner` binary the build scripts run. |
| `libvulkan-dev` + `libvulkan1` | wgpu's Vulkan backend. The loader (`libvulkan.so.1`) is a **runtime** requirement and WSL2 images frequently ship an ICD *without* it. |
| `mesa-vulkan-drivers` | the `dzn` (Vulkan-on-D3D12) and `lvp` (lavapipe software) ICDs — the fallbacks when there is no working vendor ICD. |
| `libasound2-dev`, `libssl-dev` | reachable from the Zed workspace crates. |

`cmake` and `clang` are **not** needed for the crates M0 actually compiles
(`onig_sys` builds with `cc`), despite what Zed's own build docs suggest. Add
them only if a future feature demands them.

Ubuntu 22.04 naming: `libfontconfig-dev` is the real package (`libfontconfig1-dev`
is a transitional alias) and `libfreetype-dev` supersedes `libfreetype6-dev`.

### macOS (arm64)

Xcode command line tools (`xcode-select --install`) and nothing else — Metal,
CoreText and AppKit come with the SDK. `gpui_macos` is a macOS-only path
dependency and is not compiled on Linux. **Not yet exercised: no macOS host has
run this build. M0-05 is open.**

---

## 2. First build

```
git clone <repo> && cd superterminal
git submodule update --init                                        # vendor/gpuix
git -C vendor/gpuix submodule update --init --depth 1 --recursive zed
just vendor-patch                                                  # patches/000*.patch
cd crates/st-native && cargo build --release
```

`vendor/gpuix/zed` is ~92 MB shallow. Do **not** drop `--depth 1`: a full Zed
clone is several GB and nothing here reads its history.

### `just vendor-patch` semantics

Two patches, applied in order, and `git apply` is not idempotent — running it
twice fails with `patch does not apply`. Guard each by testing the reverse first:

```
for p in patches/*.patch; do
  git -C vendor/gpuix apply --check --reverse "../../$p" 2>/dev/null \
    && echo "already applied: $p" \
    || git -C vendor/gpuix apply "../../$p"
done
```

To start over: `git -C vendor/gpuix checkout -- packages/native/src`.

> **`justfile` needs updating (owner: repo).** The current `vendor-patch` recipe
> applies only `0001` and fails on a second run. It should be the loop above.

What the patches do and why each hunk exists is in `docs/PINS.md` §6. The short
version: **0001** is the factory-registration hook plus four visibility changes;
**0002** is an upstream bug fix without which every `simulateClick` panics the
GPUI thread on Linux, so no automated input test can run.

### Build times

Measured, not estimated. Full table with artifact sizes: `docs/PINS.md` §7.
Host: WSL2, 12 threads, 7 GB RAM, `-j 6`.

| What | Time |
|---|---|
| `cargo fetch` (cold registry) | 17 s |
| `gpuix-native` cold, debug | ~3 min |
| `gpuix-native` warm (one file touched) | 6–13 s |
| `st-native` cold, debug | ~3 min |
| `st-native` cold, **release** (`-j 4`, thin LTO) | **12 min** |
| `st-native` incremental | 4–18 s |

Use `-j 6`, not the default 12: at 7 GB RAM the Zed/GPUI crates OOM-kill rustc
at full parallelism. On a 32 GB machine use the default. Budget ~7 GB of disk
per `target/` — `st-native` does not share the vendored crate's target dir.

---

## 3. WSL2 / WSLg GPU notes

**GPUI here renders through `wgpu`** (crate `gpui_wgpu`), not blade. That matters
because wgpu will silently fall back across backends, and the backend it picks
changes what you should be debugging. It logs its choice at `INFO`:

```
RUST_LOG=gpui_wgpu=info bun your-app.tsx
...
Found 1 GPU adapter(s):
  - D3D12 (NVIDIA GeForce RTX 3060 Laptop GPU) (backend=Gl, type=Other)
Selected GPU adapter: "D3D12 (NVIDIA GeForce RTX 3060 Laptop GPU)" (Gl)
```

**What WSLg gives you.** `DISPLAY=:0` and `WAYLAND_DISPLAY=wayland-0` are set by
WSLg itself; `/mnt/wslg` is the compositor's runtime directory. `gpui_platform` is
built with both the `wayland` and `x11` features and picks Wayland first
(`WAYLAND_DISPLAY= bun …` forces the X11 path). On WSLg the Wayland server does
not do server-side decorations, so GPUI logs a fallback to client-side ones —
that message is normal, not an error.

**GPU passthrough.** `/usr/lib/wsl/lib/` is bind-mounted by WSL and holds the host
driver shims — `libd3d12.so`, `libdxcore.so`, and on an NVIDIA host `libcuda.so`
and friends. Three routes, in descending order of speed:

1. **Vendor Vulkan ICD.** On an NVIDIA host `/usr/share/vulkan/icd.d/nvidia_icd.json`
   points at `libGLX_nvidia.so.0`.
2. **Dozen (`dzn`)** — Mesa's Vulkan-on-D3D12 driver, from `mesa-vulkan-drivers`.
   The right answer on AMD/Intel hosts.
3. **GL on the WSLg D3D12 adapter** — what wgpu falls back to when no Vulkan
   backend initialises. Works, and is what this repo has actually been verified on.

**Two traps, both hit on the reference machine:**

- *An ICD JSON on disk does not mean Vulkan works.* The loader (`libvulkan.so.1`,
  package `libvulkan1`) is a separate package and is missing from many WSL2
  images even when an ICD is registered.
- *A registered ICD can be broken.* Here the NVIDIA ICD is present but its
  library does not export the entry point, and wgpu logs:

  ```
  ERROR wgpu_hal::vulkan::instance] loader_scanned_icd_add: Could not get
      'vkCreateInstance' via 'vk_icdGetInstanceProcAddr' for ICD libGLX_nvidia.so.0
  ```

  This is **not fatal** — wgpu moves on and selects the GL backend, and
  everything below still passes. Do not spend an afternoon on it.

**Check it:**

```
sudo apt-get install -y vulkan-tools
vulkaninfo --summary          # must list at least one device
ls /usr/share/vulkan/icd.d/   # which ICDs are registered
ldconfig -p | grep libvulkan  # is the loader installed at all?
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/dzn_icd.x86_64.json vulkaninfo --summary
```

**Software fallback.** Because the working backend here is **GL**,
`LIBGL_ALWAYS_SOFTWARE=1` is the relevant escape hatch on this platform — it
forces llvmpipe and will render, slowly, when the D3D12-backed GL path
misbehaves. (On a host where wgpu picks Vulkan it does nothing; use lavapipe,
`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json`, instead.) Expect
single-digit FPS either way: enough to prove the pipeline, not to run a perf gate.

**Perf-gate caveat.** WSLg composites through RDP and the verified path is GL on
D3D12. Frame times measured inside WSL2 are not comparable to bare-metal Linux
and must not decide the M2 rendering gate (`docs/plan/07-milestones.md`).

---

## 4. Running the gpuix counter (verification V1)

`@gpuix/react` `require`s `@gpuix/native`, whose napi-rs loader checks
`NAPI_RS_NATIVE_LIBRARY_PATH` **first** and `require`s that path directly
(`vendor/gpuix/packages/native/index.js`, line 64). That is the mechanism the app
uses to substitute our own `.node` (`docs/plan/04-client-native.md` §1.3), and it
is confirmed present in gpuix 0.6.0. The path must end in `.node` — Bun will not
`require` a `.so`.

```
cd vendor/gpuix
bun install
(cd packages/react && bun run build)                      # @gpuix/react ships as tsc output
cd packages/native && ./node_modules/.bin/napi build --platform --features test-support
cd ../../examples && bun counter.tsx
```

(`napi build` is a thin wrapper over `cargo build` plus a rename; plain
`cargo build` and `cp target/debug/libgpuix_native.so gpuix-native.linux-x64-gnu.node`
is equivalent and is what the timings in `docs/PINS.md` measure.)

`bun --hot counter.tsx` also works, but a Node-API addon is `dlopen`ed once per
process and cannot be reloaded, so `--hot` only re-evaluates the TSX.

### The automated version

V1 is not "does a counter count" — it is "can GPUI's event loop re-enter Bun's JS
thread through a napi `ThreadsafeFunction` without deadlocking". Clicking by hand
tests that; so does `crates/st-native/tests/v1-counter.tsx`, which does it 50
times without a human:

```
cp crates/st-native/tests/v1-counter.tsx vendor/gpuix/examples/
cd vendor/gpuix/examples && bun v1-counter.tsx
```

It drives `renderer.simulateClick(x, y)` — dispatched on the GPUI thread, which
calls back into JS to run React's `setState`, which calls back into native
`applyBatch` — and reads the result out of `getAutomationTree()`.

**Status: PASS** on WSL2/WSLg — 0 → 50 in 1.54 s, no panic, no hang.
**It requires `patches/0002`.** Without it the first synthetic click panics the
`gpuix-ui` thread with `cannot update GpuixView while it is already being
updated` and every later napi call throws "The GPUI UI thread is not running".

Two Linux limitations to know before you write tests against this:

- `getPaintedText()` and `getAllText()` return `[]`, and `captureScreenshot()`
  refuses with "needs a test-support build on macOS or Windows".
  `getAutomationTree()` (types, ids, painted bounds) is the only introspection
  that works here. `TestGpuixRenderer` is compiled on macOS/Windows only.
- The automation tree carries **no text** for custom elements, only bounds. Assert
  on geometry, or on a built-in `text` node.

---

## 5. Building and loading `crates/st-native`

```
cd crates/st-native
cargo build --release                       # produces target/release/libst_native.so
mkdir -p dist && cp target/release/libst_native.so dist/superterminal-native.linux-x64-gnu.node
```

or, with the napi CLI (which also generates `index.js`/`index.d.ts`):

```
../../vendor/gpuix/packages/native/node_modules/.bin/napi build --platform --release \
  --output-dir ../../packages/app/native
```

Then point Bun at it:

```
NAPI_RS_NATIVE_LIBRARY_PATH=$PWD/dist/superterminal-native.linux-x64-gnu.node \
  bun -e 'console.log(Object.keys(require(process.env.NAPI_RS_NATIVE_LIBRARY_PATH)))'
# => [ "GpuixRenderer" ]   — identical to stock gpuix-native on Linux
```

That list must match what stock `gpuix-native` exports. Those classes come from
`gpuix-native` linked as an **rlib**, and their napi registrations are
`.init_array` constructors inside that archive; a linker pulls an object out of
an archive only when a symbol from it is already needed.
`crates/st-native/Cargo.toml` therefore pins `codegen-units = 1` for the
`gpuix-native` package, making the whole crate one object file so the
`pub use gpuix_native::*` reference drags every registration in with it. **If the
class list ever comes back empty after a dependency bump, check that setting
first** (`nm -D --defined-only *.node | grep -c napi`).

### Verifying `<hello-box>` (M0-08)

```
cp crates/st-native/tests/hello-box.tsx vendor/gpuix/examples/
cd vendor/gpuix/examples
NAPI_RS_NATIVE_LIBRARY_PATH=../../../crates/st-native/dist/superterminal-native.linux-x64-gnu.node \
  bun hello-box.tsx
```

**Status: PASS.** The element lays out at 96×47 px, and changing `label` re-shapes
the text run to 163 px wide, which is the observable proof that a prop reached
`HelloBox::set_prop` and forced a relayout. `color` cannot be read back (a quad
carries no text and screenshots are macOS/Windows-only), so the test asserts
instead that 20 rapid colour changes do not take the UI thread down.

The colour parser has a Rust unit test. `cargo test` works even though the crate
is `crate-type = ["cdylib"]` — cargo builds the lib target a second time as a
test harness:

```
cd crates/st-native && cargo test
# test hello_box::tests::parses_the_three_accepted_hex_shapes ... ok
```

---

## 6. Re-pinning gpuix (and, only with it, Zed)

`docs/PINS.md` states the rule: **never track Zed `main`.** Zed moves only when
gpuix moves, in the same commit.

```
# 1. drop our patches so the bump is a clean fast-forward
git -C vendor/gpuix checkout -- packages/native/src

# 2. move gpuix to the new release tag (tagged per npm package, not as vX.Y.Z)
git -C vendor/gpuix fetch --depth 1 origin tag '@gpuix/native@<NEW>'
git -C vendor/gpuix checkout --detach 'refs/tags/@gpuix/native@<NEW>'

# 3. let gpuix choose the Zed commit — never pick one yourself
git -C vendor/gpuix submodule update --init --depth 1 --recursive zed

# 4. rebase both patches, by hand if the hunks moved; re-check whether 0002 is
#    still needed (it is a bug fix that may have been merged upstream)
just vendor-patch

# 5. record both SHAs, the napi version and the toolchain in docs/PINS.md
git -C vendor/gpuix rev-parse HEAD
git -C vendor/gpuix submodule status zed
grep -A2 '^name = "napi"' vendor/gpuix/packages/native/Cargo.lock
cat vendor/gpuix/rust-toolchain.toml

# 6. bump @gpuix/react in the app's package.json to the SAME version
# 7. cold rebuild + rerun V1 and the hello-box check on both platforms before merging
```

CI caches `~/.cargo/registry` and `target/` keyed on the Zed SHA, so step 3
invalidates the cache on purpose; a gpuix bump PR is expected to cost a cold build.

---

## 7. If you have no root: the dev-package workaround

Recorded because it is how the numbers above were obtained, and because it will
come up again on a locked-down box or a rootless CI runner.

**Symptom.** Cold build fails after ~45 s:

```
error: failed to run custom build command for `yeslogic-fontconfig-sys v6.0.0`
  The system library `fontconfig` required by crate `yeslogic-fontconfig-sys`
  was not found. The file `fontconfig.pc` needs to be installed and the
  PKG_CONFIG_PATH environment variable must contain its parent directory.
```

**Cause.** The image has the runtime shared objects (`libfontconfig.so.1`,
`libfreetype.so.6`, `libwayland-client.so.0`, `libX11.so.6`, …) but none of the
`-dev` packages: no headers, no `.pc` files, no `.so` development symlinks.

**Fix.** The apt one-liner in §1. If `sudo -n true` fails, you cannot run it, and
this is a job for whoever owns the machine.

**Workaround.** `apt-get download` needs no root, so the dev packages can be
unpacked into a throwaway prefix:

```
SYSROOT=/tmp/gpui-sysroot
apt-get install --print-uris -y --no-install-recommends <the packages from §1> \
  | grep -oE 'http://[^"'"'"']+\.deb' | sort -u > urls.txt
xargs -a urls.txt -P8 -n1 curl -fsSLO
for d in *.deb; do dpkg-deb -x "$d" "$SYSROOT"; done

# The -dev packages' libfoo.so symlinks point at libfoo.so.N, which lives in the
# REAL /usr/lib/x86_64-linux-gnu. Inside the prefix they dangle and ld fails, so
# repoint them (repeat the loop: libpng.so -> libpng16.so -> libpng16.so.16).
cd "$SYSROOT/usr/lib/x86_64-linux-gnu"
for i in 1 2 3; do for f in *.so; do [ -L "$f" ] && [ ! -e "$f" ] \
  && ln -sf "/usr/lib/x86_64-linux-gnu/$(readlink "$f")" "$f"; done; done

export PKG_CONFIG_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$SYSROOT/usr/share/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR="$SYSROOT"     # rewrites every -I and -L into the prefix
export CPATH="$SYSROOT/usr/include:$SYSROOT/usr/include/x86_64-linux-gnu"
export LIBRARY_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu"
export LD_LIBRARY_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu:$LD_LIBRARY_PATH"
export PATH="$SYSROOT/usr/bin:$PATH"         # wayland-scanner
```

This is a verification crutch, not a supported setup: it lives outside the repo,
`PKG_CONFIG_SYSROOT_DIR` rewrites the whole build's include and link paths, and
the resulting `.node` needs `LD_LIBRARY_PATH` at run time. **CI must use real
packages.** Everything in §4 and §5 above was verified through this prefix; the
build itself is unmodified.

---

## Running it (added after the first end-to-end bring-up, 2026-08-31)

```bash
./scripts/run.sh              # build what is missing, start daemon + client
./scripts/run.sh --no-build   # run what is already built
```

`scripts/env.sh` is the single source of build/run environment and is sourced by
`run.sh`; source it yourself if you are running pieces by hand.

Four things bit us on first launch on this box. All are handled by the scripts
now, but they are worth knowing:

1. **The npm prebuilt `@gpuix/native-linux-x64-gnu` needs GLIBC 2.39** and this
   machine has 2.35, so it fails with `version 'GLIBC_2.39' not found`. Our own
   `.node` is built locally against the running glibc and is a drop-in superset.
   `env.sh` points `NAPI_RS_NATIVE_LIBRARY_PATH` at it, which gpuix's loader
   checks before anything else.
2. **The sysroot must survive a reboot.** It now lives at
   `~/.local/share/superterminal/sysroot`, not `/tmp`. `env.sh` exports both the
   build variables (`PKG_CONFIG_*`, `CPATH`, `LIBRARY_PATH`) and the runtime one
   (`LD_LIBRARY_PATH`) — the runtime one matters because `libxkbcommon-x11.so.0`
   and `libvulkan.so.1` are not installed system-wide.
3. **WSL defaults to the X11 backend.** WSLg offers Wayland and X11. Under
   Wayland, GPUI uses client-side decorations and draws no window controls, so
   the window has no close button and cannot be resized. Under X11, WSLg hands
   the window to Windows, which draws a real title bar and resize borders.
   `ST_FORCE_WAYLAND=1` opts back in.
4. **`<terminal-grid>` needs `socketPath`.** The element opens its own
   data-plane connection (Q13/Q14); without the prop the window renders chrome
   and no terminal. `st status` showing `N control, 0 data` clients is the
   symptom.

## macOS status

`cargo check --workspace --all-targets --target aarch64-apple-darwin` passes
with **zero errors** from Linux, so every non-GPU crate (`st-proto`,
`st-config`, `st-core`, `st-client-core`, `st-server`, `st-cli`) is
cfg-correct for macOS, tests included.

`crates/st-native` cannot be cross-checked from Linux: `onig_sys` (a C library
reached through gpuix's syntect dependency) needs an Apple-targeting C compiler.
That is an environment limitation, not a code problem — gpuix publishes a
darwin-arm64 prebuilt of the same dependency graph. It must be built on a Mac.

Still open on macOS, none of it verified because no host was available:
- `st_core::cwd::probe_process_cwd` returns `None`; the `proc_pidinfo` path is a
  TODO, so cwd tracking relies on OSC 7 alone.
- HANDOVER V2 (do `#[napi]` registration symbols survive `-dead_strip`?) is
  unanswered; the thin-delegate fallback in `04-client-native.md` §1.3 is the
  contingency.
- Nothing has ever been run on macOS. Treat the first launch as bring-up.
