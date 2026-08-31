# PINS — vendored dependency pins

Everything in this file is load-bearing. The rule that governs it:

> **Never track Zed `main`.** The Zed checkout under `vendor/gpuix/zed` exists only
> because gpuix path-depends on `zed/crates/gpui`. Its commit is chosen by gpuix,
> not by us. Bump it **only** as part of a gpuix bump, in the same commit, after
> re-running `patches/0001-factory-hook.patch` and a full cold build. A Zed bump
> on its own is never a valid change.

See `docs/DEV.md` for how to perform a bump and how to build.

---

## 1. gpuix (`vendor/gpuix`)

| Field | Value |
|---|---|
| Repo | `https://github.com/remorses/gpuix` |
| Release | **0.6.0** |
| Tag | `@gpuix/native@0.6.0` (identical commit as `@gpuix/react@0.6.0`) |
| Commit | `dfb83f3e096b0d3130e7b63660d9b6810cb5855c` |
| Commit date | 2026-08-29 |
| Submodule path | `vendor/gpuix` (detached HEAD at the tag) |
| Rust crate | `gpuix-native` 0.6.0 at `packages/native`, `crate-type = ["cdylib", "rlib"]` |
| npm packages | `@gpuix/native` 0.6.0, `@gpuix/react` 0.6.0 |

The repository publishes **no plain `v0.6.0` tag** — the release is tagged per
npm package (`@gpuix/native@0.6.0` / `@gpuix/react@0.6.0`), both pointing at the
same commit. Use the `@gpuix/native@` form when re-pinning.

```
git -C vendor/gpuix fetch --depth 1 origin tag '@gpuix/native@0.6.0'
git -C vendor/gpuix checkout --detach 'refs/tags/@gpuix/native@0.6.0'
```

## 2. Zed / GPUI (`vendor/gpuix/zed`)

gpuix vendors Zed as its own submodule and path-depends on it:

```toml
gpui          = { path = "../../zed/crates/gpui" }
gpui_platform = { path = "../../zed/crates/gpui_platform" }
gpui_macos    = { path = "../../zed/crates/gpui_macos" }   # macOS only
```

| Field | Value |
|---|---|
| Repo | `https://github.com/remorses/zed.git` (a **fork**, not `zed-industries/zed`) |
| Branch tracked upstream by gpuix | `gpuix` |
| Commit pinned by gpuix 0.6.0 | `8b94defe56992b3ca4ffd4853ace741d8168111a` |
| Commit date | 2026-08-27 |
| Commit subject | `gpui: resolve a negative offset_in_item in ListState::scroll_to at layout` |
| `gpui` crate version | 0.2.2 |
| Checkout size (shallow) | ~92 MB |

Initialise with:

```
git -C vendor/gpuix submodule update --init --depth 1 --recursive zed
```

`--depth 1` works because GitHub allows fetching an exact SHA; a full Zed clone
is several GB and is never needed.

## 3. napi-rs

| Component | Version |
|---|---|
| `napi` (Rust) | `3` → resolves to **3.8.3** |
| `napi-derive` | `3` → resolves to **3.5.2** |
| `napi-build` (build-dep) | `2` → resolves to **2.3.1** |
| `@napi-rs/cli` (npm, devDep of `packages/native`) | `^3.1.3` |
| napi feature set | `napi8`, `serde-json` |

`vendor/gpuix/packages/native/Cargo.lock` (lockfile v4) is committed upstream and
is the authority; `crates/st-native` keeps its own lockfile because it is a
standalone workspace (see §5).

## 4. Rust toolchain

| Where | Channel |
|---|---|
| `vendor/gpuix/rust-toolchain.toml` | **1.97.1**, profile `minimal` |
| `vendor/gpuix/zed/rust-toolchain.toml` | **1.97.1**, profile `minimal` |
| superterminal root `rust-toolchain.toml` | `stable` |

The vendored toolchain file wins for anything built inside `vendor/`, so
`rustup toolchain install 1.97.1` is a prerequisite. Verified installed here:
`rustc 1.97.1 (8bab26f4f 2026-07-14)`. The root workspace still builds on
`stable` (verified with `rustc 1.96.0`).

## 5. Workspace layout consequence

`crates/st-native` is in the root `Cargo.toml` `exclude` list and carries its own
`[workspace]` table, so `cargo build --workspace` at the root never pulls in
GPUI. Only `cd crates/st-native && cargo build` does.

## 6. Patches

Applied inside `vendor/gpuix` by `just vendor-patch`. Rebase both on every gpuix
bump. `git apply` is not idempotent — guard with `git apply --check --reverse`
first (see `docs/DEV.md` §2).

| File | Lines | What | Upstream |
|---|---|---|---|
| `patches/0001-factory-hook.patch` | 40 insertions | `register_global_factory` + the four visibility changes an out-of-tree `CustomElement` needs. Design: `docs/plan/04-client-native.md` §1.2 option (b). | PR **not yet opened** — record the URL here (M0-06 tail). |
| `patches/0002-linux-simulate-mouse-double-lease.patch` | 7 insertions | Bug fix: `simulateClick`/`simulateMouse*` panic the GPUI thread on Linux with a double lease of `GpuixView`. | Issue/PR **not yet opened**. Drop the patch when it lands. |

0001 does four things beyond adding the hook, each forced by a real compile or
test failure, not by taste:

- `pub mod custom_elements` + a `pub use` facade — otherwise the traits cannot be named.
- `pub struct GpuixView` — `CustomElement::render` takes `gpui::Context<GpuixView>`, so an out-of-tree impl must name the type.
- `pub fn apply_interactive_styles` — otherwise our element silently ignores the JSX `style` prop that every built-in honours.
- `pub mod automation` — otherwise our element never appears in `getAutomationTree()` and no JS-side test can assert on it.

0002 is a genuine upstream bug, not an accommodation for us: `UiCommand::DispatchMouse`
dispatches inside `WindowHandle::<GpuixView>::update`, which leases the view,
while gpuix's own root MouseUp handler (`text::paint::register_drag_listeners`)
updates the same view. Every synthetic left click panics `gpuix-ui` with
`cannot update GpuixView while it is already being updated`, after which every
napi call fails with "The GPUI UI thread is not running". The `DispatchKey` arm
directly below already routes through `gpui::AnyWindowHandle`, which does not
lease; the fix makes the mouse arm do the same. gpuix's own suite misses it
because `TestGpuixRenderer` is compiled on macOS/Windows only.

## 7. Build timings

Host **wsl2-3060**: WSL2, Ubuntu 22.04, kernel `6.18.33.2-microsoft-standard-WSL2`,
12 hardware threads, **7 GB RAM**, `rustc 1.97.1`, NVMe. Debug builds `-j 6`
(the default `-j 12` OOMs rustc at 7 GB), release `-j 4`.

| What | Profile | Time |
|---|---|---|
| `cargo fetch` for `gpuix-native`, cold registry | — | 17 s |
| `gpuix-native` cold (GPUI + ~700 crates) | debug | **3 min 1 s** (45 s of it spent before the fontconfig failure, then 2 min 16 s) |
| `gpuix-native` warm, touch `custom_elements/mod.rs` | debug | 13 s |
| `gpuix-native` warm, touch `renderer.rs` | debug | 6–12 s |
| `st-native` cold (own target dir, so GPUI again) | debug | **2 min 59 s** |
| `st-native` incremental after a `gpuix-native` change | debug | 18 s |
| `st-native` incremental after a `hello_box.rs` change | debug | 4–14 s |
| `st-native` cold (`-j 4`, `lto = "thin"`) | release | **12 min 2 s** |

Artifact sizes: `libst_native.so` **249 MB** release / 674 MB debug;
`libgpuix_native.so` 674 MB debug, `libgpuix_native.rlib` 93 MB.
`target/` reaches ~7 GB per crate — budget disk for two of them, since
`st-native` does not share the vendored crate's target directory.

The release build is the one that hurts: 12 minutes wall, ~40 minutes CPU, and
it was run at `-j 4` because thin LTO over the GPUI graph will not fit in 7 GB
at higher parallelism. `sccache` and `mold`/`lld` are not installed here and
would both help (`docs/plan/04-client-native.md` §2).

macOS arm64: **not measured — no macOS host was available. M0-05 is open.**

**Caveat on how these were obtained.** wsl2-3060 has no `-dev` packages installed
and no password-less `sudo`, so the apt one-liner in `docs/DEV.md` §1 could not be
run. The builds went through a throwaway `apt-get download` prefix with
`PKG_CONFIG_SYSROOT_DIR` pointed at it (`docs/DEV.md` §7). The source tree and the
dependency graph are unmodified, so the timings are representative, but the first
person with root on a normal box should re-measure and replace this table.

## 8. Verification status (HANDOVER §5)

| ID | What | Result |
|---|---|---|
| V1 | gpuix counter under Bun 1.4.0, `ThreadsafeFunction` re-entry | **PASS** on wsl2-3060. 50 `simulateClick`s through GPUI's input pipeline take the counter 0 → 50 in 1.54 s, no panic, no hang. Requires patch 0002. |
| M0-08 | `<hello-box>` from `crates/st-native` | **PASS** on wsl2-3060. Factory created, `color` and `label` props reach `set_prop`, element lays out at 96×47 px and re-shapes to 163 px wide when `label` changes; survives 20 colour changes. |
| — | `.node` loads under Bun | **PASS**. `require()` on `libst_native.so` renamed `.node` exports `GpuixRenderer`, identical to stock `gpuix-native`; the `codegen-units = 1` pin on the `gpuix-native` package keeps the rlib's napi `.init_array` registrations alive. |
| — | Window opens under WSLg | **PASS**, Wayland backend, client-side decorations. |
| — | Release build | **PASS**. `cargo build --release` produces a 249 MB `.node` that loads under Bun and passes the `<hello-box>` check with the same geometry as the debug build. |

**Renderer detail worth pinning:** this GPUI renders through **wgpu**
(`gpui_wgpu`), not blade. On wsl2-3060 the Vulkan ICD is broken
(`libGLX_nvidia.so.0` does not export `vk_icdGetInstanceProcAddr`), so wgpu falls
back to the **GL** backend on the WSLg D3D12 adapter and reports
`Selected GPU adapter: "D3D12 (NVIDIA GeForce RTX 3060 Laptop GPU)" (Gl)`.
Every frame time measured on this host is therefore a GL-on-D3D12-on-RDP number
and must not be the one that decides the M2 rendering gate.
