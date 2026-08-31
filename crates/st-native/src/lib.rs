//! superterminal's Node-API addon.
//!
//! One cdylib for the process (`docs/plan/04-client-native.md` §1.3, option B):
//! `gpuix-native` is linked in as an **rlib**, so there is exactly one GPUI
//! runtime, one `CustomElementRegistry`, and one `.node` for Bun to `dlopen`.
//! Loading the stock `@gpuix/native` addon *alongside* this one would give the
//! process two of each and put our factories in the registry nobody renders
//! from — the app therefore points `NAPI_RS_NATIVE_LIBRARY_PATH` at this file
//! before `@gpuix/react` is evaluated (`docs/plan/05-client-app.md` §8).
//!
//! # Module map (04 §11)
//!
//! | module | purpose |
//! |---|---|
//! | [`element`] | the `<terminal-grid>` `CustomElement` and its factory |
//! | [`paint`] | one frame: quads, runs, decorations, cursor, scrollbar |
//! | [`runs`] | row → background spans + style runs, and the shaped-line cache |
//! | [`geometry`] | cell metrics, grid sizing, scroll and scrollbar arithmetic |
//! | [`props`] | the JSON prop surface, validation, the passthrough matcher |
//! | [`theme`] | `theme` prop → `st_client_core::Palette` |
//! | [`input`] | gpui keystrokes → PTY bytes |
//! | [`mouse`] | pointer glue: hit zones, wheel accumulation, motion throttle |
//! | [`conn`] | one shared Data Plane connection per socket |
//! | [`wake`] | waking GPUI from the Data Plane thread |
//! | [`viewstate`] | `SetViewState` payloads and debouncing |
//! | [`registry`] | published element state behind `stReadProp` |
//! | [`stats`] | frame timings and cache hit rate (04-OQ10) |
//!
//! # HANDOVER V5 — how a declined chord reaches React
//!
//! **Verified on Linux/WSLg, 2026-08-31, by `tests/passthrough-keys.tsx`:**
//! a chord listed in `passthroughKeys` is declined by *not* calling
//! `cx.stop_propagation()`, GPUI keeps bubbling it up the focus chain, and the
//! React ancestor's `onKeyDown` fires. **Bubbling is the mechanism in use.**
//! The same test confirms the inverse: `ctrl-c`, which is not in the list, is
//! consumed and never reaches the ancestor.
//!
//! The element *also* emits a `shortcut` event carrying the chord, as the
//! fallback HANDOVER §5 asks for. It works, but only if the listener is
//! registered by hand — `@gpuix/react`'s `EVENT_PROPS` is a closed list with no
//! `shortcut` entry, so JSX `onShortcut` never reaches
//! `setEventListener`. A one-line addition to `packages/react` would fix that;
//! it is not needed while bubbling works.
//!
//! # Known gap: `SetViewState` has no transport
//!
//! Grilling Q43/Q49 route selection and scroll offset over the Data Plane as
//! message `0x0016`, but `st-client-core`'s `DataPlaneHandle` has no method to
//! send one and its socket writer is private. [`viewstate`] therefore builds
//! and debounces the message correctly and hands it to a pluggable sink whose
//! default only records it. Closing the gap is three lines in
//! `crates/st-client-core/src/dataplane.rs`:
//!
//! ```ignore
//! pub fn set_view_state(&self, message: SetViewState) -> Result<(), DataPlaneError> {
//!     self.shared.send(&DataMsg::SetViewState(message))
//! }
//! ```
//!
//! …after which `st_native::viewstate::set_sink` gets an implementation that
//! calls it. Nothing else in this crate changes.

#![deny(missing_docs)]

pub mod conn;
pub mod element;
pub mod geometry;
pub mod hello_box;
pub mod input;
pub mod mouse;
pub mod paint;
pub mod props;
pub mod registry;
pub mod runs;
pub mod stats;
pub mod theme;
pub mod viewstate;
pub mod wake;

/// Re-export gpuix's whole napi surface. Two jobs at once: JS gets the
/// `GpuixRenderer` / `TestGpuixRenderer` classes `@gpuix/react` expects, and the
/// reference keeps the linker from dropping the rlib's `.init_array`
/// registration statics (see the `codegen-units = 1` note in Cargo.toml).
pub use gpuix_native::*;

/// Runs when Bun `dlopen`s this addon, which is strictly before any JS can call
/// `new GpuixRenderer(...).init(...)`. That ordering is the whole reason the
/// gpuix hook is a global registry rather than an `init` argument: by the time
/// `GpuixView::new` calls `CustomElementRegistry::with_defaults`, our
/// constructors are already in the list.
#[napi_derive::module_init]
fn register_superterminal_elements() {
    gpuix_native::register_global_factory(|| Box::new(hello_box::HelloBoxFactory));
    gpuix_native::register_global_factory(|| Box::new(element::TerminalGridFactory));
}

/// Reads a `<terminal-grid>` property that is state rather than a prop
/// (04 §3, "Imperative reads").
///
/// gpuix has no `get_prop` over napi and the patch budget is spent
/// (invariant I5), so this is our own accessor over the snapshot every element
/// publishes at the end of each frame. Keyed on Surface id because that is
/// what React holds.
///
/// Keys: `scrollOffset`, `contentLines`, `viewportTop`, `title`,
/// `selectionText`, `hasSelection`, `cellSize`, `size`, `connected`,
/// `attached`, `elementId`, `modes`, `stats`, `viewState`. `null` for an
/// unknown key or an unmounted Surface.
#[napi_derive::napi(js_name = "stReadProp")]
pub fn st_read_prop(surface_id: u32, key: String) -> Option<serde_json::Value> {
    registry::snapshot_for_surface(st_proto::SurfaceId(surface_id))?.read(&key)
}

/// Every Surface with a mounted `<terminal-grid>`, for tests and diagnostics.
#[napi_derive::napi(js_name = "stListGrids")]
pub fn st_list_grids() -> Vec<u32> {
    registry::published_surfaces()
}

/// Every key [`st_read_prop`] answers.
#[napi_derive::napi(js_name = "stReadableProps")]
pub fn st_readable_props() -> Vec<String> {
    registry::READABLE_PROPS
        .iter()
        .map(|key| (*key).to_string())
        .collect()
}

/// Opens the Data Plane socket ahead of the first `<terminal-grid>` mount, so
/// the handshake overlaps with the first React render instead of following it
/// (04 §5). Returns the error string on failure; the element retries on its
/// next frame either way.
#[napi_derive::napi(js_name = "stConnectDataPlane")]
pub fn st_connect_data_plane(path: String, build_id: Option<String>) -> napi::Result<bool> {
    let build_id = build_id.unwrap_or_else(|| format!("st-native/{}", env!("CARGO_PKG_VERSION")));
    match conn::open(&path, &build_id) {
        Ok(plane) => {
            conn::pin(&plane);
            Ok(true)
        }
        Err(error) => Err(napi::Error::from_reason(error)),
    }
}

/// The Data Plane sockets this process has open.
#[napi_derive::napi(js_name = "stDataPlanePaths")]
pub fn st_data_plane_paths() -> Vec<String> {
    conn::open_paths()
}
