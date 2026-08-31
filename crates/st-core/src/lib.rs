//! `st-core` — the Superterminal Server's terminal engine, with no runtime.
//!
//! Everything the Server does to a Surface that is *not* async I/O lives here:
//! the VT state machine behind a swappable trait, PTY spawn and control, style
//! interning, Snapshot/Delta production and the fan-out state machine. There is
//! deliberately no tokio, no GPUI and no napi (`docs/plan/01-architecture.md`),
//! so every rule in this crate is unit-testable without a runtime and without a
//! window.
//!
//! ```text
//!  PTY bytes ──► Surface::feed ──► VtEngine ──► Damage
//!                                     │
//!                    SurfaceStyleTable │ (interning, Q45 cap)
//!                                     ▼
//!            Publisher::flush(now) ──► Snapshot / Delta per Client
//! ```
//!
//! # Modules
//!
//! * [`vt`] — the [`VtEngine`] trait, [`Damage`] and
//!   the `alacritty_terminal` implementation (the only place that crate is
//!   named — invariant I6).
//! * [`style_table`] — per-Surface [`Style`](st_proto::Style) interning and the
//!   grilling-Q45 overflow policy.
//! * [`pty`] — `portable-pty` spawn, resize, wait and `SIGHUP`-the-group kill.
//! * [`cwd`] — OSC 7 tracking plus the `/proc` fallback probe.
//! * [`surface`] — the Surface engine object that ties the four together and
//!   produces `st-proto` frames.
//! * [`publisher`] — per-Surface fan-out: coalescing, the 120 Hz emit rule, the
//!   ack window and the slow-client forced Snapshot.
//!
//! # Frozen decisions this crate implements
//!
//! | Rule | Source |
//! |---|---|
//! | Row-granular damage; a Delta carries whole rows | Q16 |
//! | `history_len: u64` in `AbsLine` units, plus `history_base` | Q39 |
//! | Reflow off on resize, selection cleared, last resize wins | Q40 |
//! | Trailing blanks trimmed, per-row `wrapped` flag | Q41 |
//! | Style table capped at 4096, overflow ⇒ reset + Snapshot | Q45 |
//! | OSC 52 off; `alacritty_terminal` from crates.io 0.26.x | Q48 |

#![deny(missing_docs)]
#![forbid(unsafe_op_in_unsafe_fn)]

pub mod cwd;
pub mod pty;
pub mod publisher;
pub mod style_table;
pub mod surface;
pub mod vt;

pub use cwd::{CwdTracker, Osc7Sniffer};
pub use pty::{ExitStatus, Pty, PtyConfig, PtyError};
pub use publisher::{
    ClientId, Coalesced, Emission, EmissionKind, Publisher, PublisherConfig, Subscription,
};
pub use style_table::SurfaceStyleTable;
pub use surface::{ClientFrame, Surface, SurfaceConfig, SurfaceStatus, SurfaceUpdate};
pub use vt::alacritty::{AlacrittyEngine, EngineConfig};
pub use vt::{Damage, DirtySet, GridSnapshot, VtEngine, VtEvent};
