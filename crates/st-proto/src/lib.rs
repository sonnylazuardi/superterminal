//! Superterminal wire protocol v1.0 — the single source of truth for both
//! planes of the Client ↔ Server link.
//!
//! The authoritative specification is
//! [`docs/plan/02-protocol.md`](https://github.com/sonnylazuardi/superterminal/blob/main/docs/plan/02-protocol.md),
//! amended by section F (Q37–Q48) of `docs/plan/00-grilling.md`. Section
//! numbers in the doc comments below refer to that spec.
//!
//! # The two planes
//!
//! Every connection is CONTROL or DATA for its whole lifetime, decided by its
//! first byte ([`frame::detect_connection_kind`]):
//!
//! | Plane | First byte | Codec | Module | Speaker |
//! |---|---|---|---|---|
//! | CONTROL | `{` | newline-delimited JSON | [`control`] | the Bun app and the `st` CLI |
//! | DATA | `0xFF` + `"STD"` | `u32 len \| u16 msg_type \| postcard` | [`data`] | the Rust native client |
//!
//! # Layout
//!
//! * [`ids`] — [`SurfaceId`], [`SessionId`], [`TabId`], [`Seq`], [`AbsLine`],
//!   [`StyleIdx`].
//! * [`cell`] — [`PackedCell`], [`Row`], [`Style`], [`StyleTable`] and the
//!   flag sets, i.e. everything grid-shaped (§5).
//! * [`control`] — requests, responses and events of the JSON plane (§3).
//! * [`data`] — the binary plane's messages and their `msg_type` table (§4).
//! * [`frame`] — framing, the magic, and the [`Hello`]/[`HelloAck`]/[`Reject`]
//!   handshake (§1–§2).
//!
//! # Sending a data-plane message
//!
//! ```
//! use st_proto::{DataMsg, FrameDecoder, Input, SurfaceId};
//!
//! let msg = DataMsg::Input(Input { surface_id: SurfaceId(9), bytes: b"ls\r".to_vec() });
//!
//! let mut wire = Vec::new();
//! msg.encode_to(&mut wire)?;
//!
//! let mut decoder = FrameDecoder::new();
//! decoder.push(&wire);
//! let frame = decoder.next_frame()?.expect("one complete frame");
//! assert_eq!(DataMsg::from_frame(frame.msg_type, &frame.payload)?, msg);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Compatibility
//!
//! JSON is self-describing, so adding an optional field or a new request is a
//! minor change; postcard is positional, so a changed struct layout needs a
//! new `msg_type` (§10). Reserved bits of [`Modes`], [`Attrs`] and
//! [`CellFlags`] are masked off on decode rather than rejected, which is what
//! makes "new bit positions" a minor change too.
//!
//! Per invariant I8 of `HANDOVER.md` this crate depends only on `serde`,
//! `postcard`, `serde_json`, `bitflags` and `thiserror`: no async runtime, no
//! terminal engine, no UI toolkit.

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

/// Serialize a `bitflags` type as its raw bits, and deserialize with
/// `from_bits_truncate` so that reserved bits set by a newer peer are masked
/// off instead of failing the decode (`02-protocol.md` §10).
macro_rules! impl_flags_serde {
    ($name:ident, $repr:ty) => {
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serde::Serialize::serialize(&self.bits(), serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let bits = <$repr as serde::Deserialize>::deserialize(deserializer)?;
                Ok(Self::from_bits_truncate(bits))
            }
        }
    };
}

pub mod cell;
pub mod control;
pub mod data;
pub mod frame;
pub mod ids;

pub use cell::{Attrs, CellFlags, Color, PackedCell, Row, Style, StyleTable, STYLE_TABLE_CAP};
pub use control::{
    AbsPoint, AnyRes, ControlMsg, Empty, ErrRes, ErrorBody, ErrorCode, Ev, Handshake, KillSignal,
    OkRes, Req, ReqId, Res, Revision, RevisionResult, Selection, SelectionKind, ServerStatus,
    Session, SessionCreated, SessionList, SpawnSpec, SurfaceCreated, SurfaceMeta, SurfaceState,
    Tab, TabCreated, ViewState, Workspace, WorkspaceSnapshot, DEFAULT_ENV_ALLOW_LIST,
};
pub use data::{
    msg_type, Ack, Attach, AttachMode, Bell, CodecError, Cursor, CursorShape, DataError, DataMsg,
    Delta, Detach, DetachReason, Detached, DirtyRow, ExitStatus, FetchHistory, History, Input,
    Modes, Resize, SetViewState, Snapshot, SurfaceExited, DATA_ERR_BAD_REQUEST,
    DATA_ERR_NOT_ATTACHED, DATA_ERR_SURFACE_EXITED, MAX_HISTORY_COUNT, MAX_INPUT_BYTES,
    MAX_UNACKED_DELTAS,
};
pub use frame::{
    detect_connection_kind, encode_frame, encode_frame_to_vec, ClientKind, ConnectionKind, Frame,
    FrameDecoder, FrameError, Hello, HelloAck, ProtoVersion, Reject, RejectReason,
    CONTROL_FIRST_BYTE, DATA_MAGIC, FRAME_HEADER_LEN, MAX_CONTROL_LINE, MAX_FRAME, MAX_PAYLOAD,
    PROTO_VERSION,
};
pub use ids::{AbsLine, Seq, SessionId, StyleIdx, SurfaceId, TabId};
