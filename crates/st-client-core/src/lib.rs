//! The pure-logic half of the Superterminal client — `docs/plan/04-client-native.md` §11.
//!
//! Everything between the Data Plane socket and the pixels, minus the pixels.
//! Per **invariant I9** this crate has **no GPUI dependency** and is fully
//! unit-testable on a headless box: `st-native` owns the GPUI element and
//! calls into here for every decision it makes.
//!
//! ```text
//!  server ──frames──► dataplane ──apply──► replica ──rows──► st-native (GPUI)
//!                          ▲                  │                    │
//!                          │                  ├─ selection ────────┤
//!                          └──── keys/mouse ──┴─ palette ──────────┘
//! ```
//!
//! # Modules
//!
//! | module | what it owns |
//! |---|---|
//! | [`replica`] | [`Replica`], Snapshot/Delta/History application, [`Gap`] detection, history caching |
//! | [`keys`] | xterm key encoding, bracketed paste |
//! | [`mouse`] | X10/1000/1002/1003 and SGR reporting, wheel handling |
//! | [`selection`] | selection model in absolute line coordinates, text extraction |
//! | [`palette`] | the 256-colour table, `bold_is_bright`, dim and inverse resolution |
//! | [`dataplane`] | the socket client: handshake, framing, the Replica map, wake callback |
//!
//! # The invariants this crate depends on
//!
//! * **I1** — the Server owns the terminal state machine. Nothing here parses
//!   VT bytes; a [`Replica`] is only ever mutated by an inbound message.
//! * **I8** — [`st_proto`] is the single source of wire truth. Types this
//!   crate needs and `st-proto` does not have (a platform-neutral
//!   [`KeyEvent`](keys::KeyEvent), a [`Palette`](palette::Palette)) are
//!   defined here instead of being pushed onto the wire crate.
//! * **I9** — no GPUI, no napi, no GPU. Colours come out as `(u8, u8, u8)`;
//!   key events come in as a local enum.
//!
//! # A tour
//!
//! ```no_run
//! use st_client_core::dataplane::{DataPlaneConnection, DataPlaneOptions};
//! use st_client_core::keys::{encode_key, Key, KeyConfig, KeyEvent, Mods};
//! use st_proto::{AttachMode, SurfaceId};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // One socket, one dedicated I/O thread, one wake callback.
//! let connection = DataPlaneConnection::connect(
//!     "/run/user/1000/superterminal/data.sock",
//!     DataPlaneOptions::default(),
//!     Box::new(|| { /* schedule a GPUI repaint */ }),
//! )?;
//!
//! let surface = SurfaceId(1);
//! connection.attach(surface, AttachMode::Active)?;
//!
//! // A keystroke becomes PTY bytes here, never on the server.
//! let modes = connection.with_replica(surface, |r| r.modes()).unwrap_or_default();
//! if let Some(bytes) = encode_key(&KeyEvent::new(Key::Char('c'), Mods::CTRL), modes, &KeyConfig::default()) {
//!     connection.send_input(surface, &bytes)?;
//! }
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod dataplane;
pub mod keys;
pub mod mouse;
pub mod palette;
pub mod replica;
pub mod selection;

pub use dataplane::{
    DataPlaneCore, DataPlaneError, DataPlaneEvent, DataPlaneHandle, DataPlaneOptions, Shared,
    WakeFn,
};
pub use keys::{encode_key, prepare_paste, Key, KeyConfig, KeyEvent, Keypad, Mods};
pub use mouse::{
    encode_mouse, handle_wheel, AltScreenScroll, MouseButton, MouseEncoding, MouseEvent,
    MouseEventKind, MouseProtocol, WheelAction, WheelConfig,
};
pub use palette::{Palette, ResolvedStyle, Rgb};
pub use replica::{Gap, Replica, ReplicaConfig, DEFAULT_HISTORY_CAP};
pub use selection::{AbsPoint, Selection, SelectionConfig, SelectionMode};

#[cfg(unix)]
pub use dataplane::DataPlaneConnection;

/// Everything `st-native` typically imports.
pub mod prelude {
    pub use crate::dataplane::{DataPlaneEvent, DataPlaneHandle, DataPlaneOptions};
    pub use crate::keys::{encode_key, Key, KeyConfig, KeyEvent, Mods};
    pub use crate::mouse::{encode_mouse, handle_wheel, MouseEvent, MouseProtocol, WheelAction};
    pub use crate::palette::{Palette, ResolvedStyle, Rgb};
    pub use crate::replica::Replica;
    pub use crate::selection::{Selection, SelectionConfig, SelectionMode};
    pub use st_proto::{AbsLine, AttachMode, Modes, Row, Seq, SurfaceId};
}
