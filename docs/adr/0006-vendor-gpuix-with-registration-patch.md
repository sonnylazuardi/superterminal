---
status: accepted
---
# Vendor gpuix as a submodule with a minimal factory‑registration patch; build our own Node‑API module

gpuix 0.6.0 registers custom elements only inside `GpuixRenderer::init` via `with_defaults()`; downstream registration ("phase 2 plugins") is planned but not implemented, and GPUI itself is a path dependency on a vendored Zed checkout. We decided to vendor gpuix at `vendor/gpuix`, pin the Zed commit it pins, carry a ~30‑line patch that lets a downstream crate register additional `CustomElementFactory` implementations, and ship our own `@superterminal/native` `.node` built from a crate depending on `gpuix-native`. We will upstream the hook; until then the patch is ours to maintain.

## Considered options
- Wait for upstream plugin loading — rejected: unknown timeline; blocks M0.
- Render the grid via `<code>`/`<text>` elements — rejected: see ADR‑0005.
- Fork GPUI directly without gpuix — rejected: loses the React chrome, the whole point of the stack.

## Consequences
- First native build compiles GPUI from source (10–25 min cold; cached after). CI must cache the submodule build.
- Never track Zed `main`; bump only together with a gpuix bump.
