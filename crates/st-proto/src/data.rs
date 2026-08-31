//! Data-plane messages — `docs/plan/02-protocol.md` §4, §6–§9.
//!
//! Every message is postcard-encoded and carried in one frame (see
//! [`crate::frame`]); the frame's `msg_type` selects the struct. Postcard is
//! not self-describing, so the layout of each struct is fixed per
//! `(major, minor)` — see §10 for what may change in a minor.
//!
//! Amendments from grilling §F applied here:
//!
//! * Q38 — `Delta` carries [`since_seq`](Delta::since_seq); the standalone
//!   `ModeChanged` (`0x0104`) and `TitleChanged` (`0x0105`) messages are gone,
//!   modes and title ride inside the `Delta`.
//! * Q39 — `scrollback_appended: u32` is replaced by the absolute
//!   `history_len: u64` on both `Snapshot` and `Delta`.
//! * Q44 — `Attach` carries an [`AttachMode`].
//! * Q48 — `DataError` is per message, never connection-fatal.

use serde::{Deserialize, Serialize};

use crate::cell::{Row, Style};
use crate::control::ViewState;
use crate::frame::{encode_frame, FrameError, Hello, HelloAck, Reject};
use crate::ids::{AbsLine, Seq, StyleIdx, SurfaceId};

/// `msg_type` values (`02-protocol.md` §4.1).
///
/// `0x0000–0x00FF` is client → server (plus the handshake), `0x0100–0x01FF` is
/// server → client. Future minors allocate upward.
pub mod msg_type {
    /// [`crate::frame::Hello`], C→S.
    pub const HELLO: u16 = 0x0001;
    /// [`crate::frame::HelloAck`], S→C.
    pub const HELLO_ACK: u16 = 0x0002;
    /// [`crate::frame::Reject`], S→C.
    pub const REJECT: u16 = 0x0003;

    /// [`super::Attach`], C→S.
    pub const ATTACH: u16 = 0x0010;
    /// [`super::Detach`], C→S.
    pub const DETACH: u16 = 0x0011;
    /// [`super::Input`], C→S.
    pub const INPUT: u16 = 0x0012;
    /// [`super::Resize`], C→S.
    pub const RESIZE: u16 = 0x0013;
    /// [`super::FetchHistory`], C→S.
    pub const FETCH_HISTORY: u16 = 0x0014;
    /// [`super::Ack`], C→S.
    pub const ACK: u16 = 0x0015;
    /// [`SetViewState`](super::SetViewState) — client reports selection / scroll offset.
    ///
    /// Q43: View State edits travel on the data plane because the Rust element
    /// produces them from the [`Replica`](../../st_client_core/replica/struct.Replica.html);
    /// routing them through JS would add a napi hop for nothing. The server stores
    /// them on the Surface and echoes them on the control plane in `ev.workspace`.
    pub const SET_VIEW_STATE: u16 = 0x0016;

    /// [`super::Snapshot`], S→C.
    pub const SNAPSHOT: u16 = 0x0100;
    /// [`super::Delta`], S→C.
    pub const DELTA: u16 = 0x0101;
    /// [`super::History`], S→C.
    pub const HISTORY: u16 = 0x0102;
    /// [`super::SurfaceExited`], S→C.
    pub const SURFACE_EXITED: u16 = 0x0103;
    /// Retired: `ModeChanged` (grilling Q38). Never sent or accepted in 1.x.
    pub const RESERVED_MODE_CHANGED: u16 = 0x0104;
    /// Retired: `TitleChanged` (grilling Q38). Never sent or accepted in 1.x.
    pub const RESERVED_TITLE_CHANGED: u16 = 0x0105;
    /// [`super::Bell`], S→C.
    pub const BELL: u16 = 0x0106;
    /// [`super::Detached`], S→C.
    pub const DETACHED: u16 = 0x0107;
    /// [`super::DataError`], S→C.
    pub const DATA_ERROR: u16 = 0x01FF;
}

/// Largest `Input.bytes` the client may put in one frame (§4.2); longer pastes
/// are chunked by the client.
pub const MAX_INPUT_BYTES: usize = 64 * 1024;

/// Largest `FetchHistory.count` the server will honour (§8, grilling Q25).
pub const MAX_HISTORY_COUNT: u16 = 1000;

/// Deltas in flight per (client, Surface) before the server must wait for an
/// [`Ack`] (§6.5).
pub const MAX_UNACKED_DELTAS: u32 = 4;

/// [`DataError::code`]: input was sent to an exited Surface (§9, grilling Q48).
pub const DATA_ERR_SURFACE_EXITED: u16 = 0x0001;

/// [`DataError::code`]: the Surface id is unknown, or the connection is not
/// attached to it.
pub const DATA_ERR_NOT_ATTACHED: u16 = 0x0002;

/// [`DataError::code`]: the message was malformed or violated a documented
/// limit (oversized `Input`, `count` above [`MAX_HISTORY_COUNT`], …).
pub const DATA_ERR_BAD_REQUEST: u16 = 0x0003;

// ---------------------------------------------------------------- client → server

/// How much of a Surface a client wants (grilling Q44).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttachMode {
    /// The visible Tab: rows, cursor, modes, title, everything.
    Active,
    /// A background Tab in the active Session: title, exit, bell and
    /// `history_len` only — no rows.
    Passive,
}

/// Subscribe this connection to a Surface (§4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attach {
    /// The Surface to attach to.
    pub surface_id: SurfaceId,
    /// How much to stream (grilling Q44).
    pub mode: AttachMode,
    /// `true`: always answer with a `Snapshot`. `false`: only when
    /// `known_seq` is stale.
    pub want_snapshot: bool,
    /// The last sequence number the client has applied; `0` = nothing known.
    pub known_seq: Seq,
}

/// Unsubscribe from a Surface (§4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detach {
    /// The Surface to detach from.
    pub surface_id: SurfaceId,
}

/// Bytes to write to the PTY, verbatim (§9).
///
/// Keys, mouse reports and paste bracketing are all encoded on the client;
/// the server never interprets these bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    /// Target Surface.
    pub surface_id: SurfaceId,
    /// Raw bytes; at most [`MAX_INPUT_BYTES`] per frame.
    pub bytes: Vec<u8>,
}

/// Change a Surface's grid size (§9). Last writer wins (grilling Q40).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resize {
    /// Target Surface.
    pub surface_id: SurfaceId,
    /// New column count.
    pub cols: u16,
    /// New row count.
    pub rows: u16,
}

/// Ask for a page of scrollback (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchHistory {
    /// Target Surface.
    pub surface_id: SurfaceId,
    /// First line wanted; the answer starts at `max(from_line, history_base)`.
    pub from_line: AbsLine,
    /// How many lines to return; at most [`MAX_HISTORY_COUNT`].
    pub count: u16,
}

/// "I have applied everything up to and including `seq`" (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ack {
    /// Target Surface.
    pub surface_id: SurfaceId,
    /// The highest sequence number applied by the client.
    pub seq: Seq,
}

// ---------------------------------------------------------------- server → client

/// Client -> server: the user's View State for a Surface (Q43).
///
/// Sent on mouse-up after a selection drag and (debounced) while scrolling.
/// The server persists it on the Surface so it survives a client relaunch,
/// and echoes it to control-plane subscribers via `ev.workspace`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetViewState {
    /// Surface whose View State is being reported.
    pub surface: SurfaceId,
    /// First visible absolute line; `None` means "pinned to the bottom".
    pub scroll_offset: Option<AbsLine>,
    /// Current selection, or `None` to clear it.
    pub selection: Option<crate::control::Selection>,
}

/// Cursor state (§4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]

pub struct Cursor {
    /// Row within the visible grid.
    pub row: u16,
    /// Column within the visible grid.
    pub col: u16,
    /// Shape requested by the program (DECSCUSR).
    pub shape: CursorShape,
    /// Whether the cursor is shown (DECTCEM, and hidden while scrolled up).
    pub visible: bool,
    /// Whether the cursor blinks.
    pub blink: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            row: 0,
            col: 0,
            shape: CursorShape::Block,
            visible: true,
            blink: true,
        }
    }
}

/// Cursor shapes (§4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum CursorShape {
    /// Filled block.
    #[default]
    Block,
    /// Underscore.
    Underline,
    /// Vertical bar.
    Beam,
}

impl_flags_serde!(Modes, u16);

bitflags::bitflags! {
    /// Terminal modes the client must know to encode input and render (§4.4).
    ///
    /// Unknown bits set by a newer peer are masked off on decode (§10).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct Modes: u16 {
        /// The alternate screen is active; there is no scrollback.
        const ALT_SCREEN = 1 << 0;
        /// Bracketed paste (mode 2004).
        const BRACKETED_PASTE = 1 << 1;
        /// Mouse click reporting (mode 1000).
        const MOUSE_CLICK = 1 << 2;
        /// Mouse drag reporting (mode 1002).
        const MOUSE_DRAG = 1 << 3;
        /// Any-motion mouse reporting (mode 1003).
        const MOUSE_MOTION = 1 << 4;
        /// SGR mouse encoding (mode 1006).
        const MOUSE_SGR = 1 << 5;
        /// Application cursor keys (DECCKM).
        const APP_CURSOR_KEYS = 1 << 6;
        /// Application keypad.
        const APP_KEYPAD = 1 << 7;
        /// Focus in/out reporting (mode 1004).
        const FOCUS_EVENTS = 1 << 8;
        /// Auto-wrap (DECAWM).
        const LINE_WRAP = 1 << 9;
        /// Kitty keyboard protocol; reserved, never emitted in 1.0.
        const KITTY_KEYBOARD = 1 << 10;
    }
}

impl Modes {
    /// Any mouse-reporting mode is on, so the client should encode mouse
    /// events into `Input` (§9).
    #[inline]
    #[must_use]
    pub const fn mouse_reporting(self) -> bool {
        self.intersects(
            Modes::MOUSE_CLICK
                .union(Modes::MOUSE_DRAG)
                .union(Modes::MOUSE_MOTION),
        )
    }
}

/// How a process ended (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExitStatus {
    /// Exit code, when it exited normally.
    pub code: Option<i32>,
    /// Signal number, when it was killed.
    pub signal: Option<i32>,
}

/// Complete Surface state; replaces the replica wholesale (§7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The Surface this describes.
    pub surface_id: SurfaceId,
    /// The sequence number of the state described.
    pub seq: Seq,
    /// Grid width.
    pub cols: u16,
    /// Grid height.
    pub rows: u16,
    /// The whole style table, in index order; index 0 is the default style.
    pub styles: Vec<Style>,
    /// Exactly `rows` entries, top to bottom.
    pub grid: Vec<Row>,
    /// Cursor state.
    pub cursor: Cursor,
    /// Terminal modes.
    pub modes: Modes,
    /// Current window title.
    pub title: String,
    /// Id of the oldest retained history line.
    pub history_base: AbsLine,
    /// Number of retained history lines; no content is carried (grilling Q39).
    pub history_len: u64,
    /// The control-plane View State at this `seq`.
    pub view_state: ViewState,
    /// `Some` when the Surface has already exited.
    pub exited: Option<ExitStatus>,
}

impl Snapshot {
    /// The absolute id of the top visible row: `history_base + history_len` (§8).
    #[inline]
    #[must_use]
    pub const fn first_visible_line(&self) -> AbsLine {
        self.history_base.saturating_add(self.history_len)
    }
}

/// An incremental update (§4.3, §6).
///
/// Contents are the full dirty set relative to the client's last acknowledged
/// state, so a coalesced client legitimately skips `seq` values;
/// [`since_seq`](Delta::since_seq) is what the gap detector compares
/// (grilling Q38).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delta {
    /// The Surface this updates.
    pub surface_id: SurfaceId,
    /// The sequence number of the resulting state.
    pub seq: Seq,
    /// The state this delta builds on. A client whose `last_seq` differs has
    /// missed something and must re-`Attach` with `want_snapshot: true`.
    pub since_seq: Seq,
    /// Post-update id of the oldest retained history line; lets the client
    /// drop cached rows below it.
    pub history_base: AbsLine,
    /// Post-update count of retained history lines. The client derives how
    /// many lines were appended or evicted (grilling Q39).
    pub history_len: u64,
    /// `Some((cols, rows))` when the grid was resized in this delta; the
    /// replica resizes before applying rows and every row arrives dirty.
    pub resized: Option<(u16, u16)>,
    /// Style-table additions, applied before the rows that reference them.
    pub new_styles: Vec<(StyleIdx, Style)>,
    /// Dirty rows, each carrying full row content (grilling Q16).
    pub rows: Vec<DirtyRow>,
    /// Cursor state after the update.
    pub cursor: Cursor,
    /// Terminal modes after the update.
    pub modes: Modes,
    /// `Some` only when the title changed (grilling Q38).
    pub title: Option<String>,
}

impl Delta {
    /// The absolute id of the top visible row after applying this delta (§8).
    #[inline]
    #[must_use]
    pub const fn first_visible_line(&self) -> AbsLine {
        self.history_base.saturating_add(self.history_len)
    }
}

/// One row of a [`Delta`] (§4.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyRow {
    /// Row index within the visible grid, 0 = top.
    pub index: u16,
    /// The complete new content of that row.
    pub row: Row,
}

/// A page of scrollback, answering [`FetchHistory`] (§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    /// The Surface this came from.
    pub surface_id: SurfaceId,
    /// Id of the first row returned: `max(request.from_line, history_base)`.
    pub from_line: AbsLine,
    /// Current trim point, so the client can drop cached rows below it.
    pub history_base: AbsLine,
    /// The rows, in increasing line order; may be shorter than requested.
    pub rows: Vec<Row>,
}

/// The Surface's process ended (§4.3). Consumes a sequence number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceExited {
    /// The Surface that exited.
    pub surface_id: SurfaceId,
    /// The sequence number of this state change.
    pub seq: Seq,
    /// How the process ended.
    pub status: ExitStatus,
}

/// The program rang the bell (§4.3). Outside the sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bell {
    /// The Surface that rang.
    pub surface_id: SurfaceId,
}

/// The server dropped this connection's attachment (§4.3). Outside the sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detached {
    /// The Surface that is no longer attached.
    pub surface_id: SurfaceId,
    /// Why.
    pub reason: DetachReason,
}

/// Why a [`Detached`] was sent (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DetachReason {
    /// The client asked, with [`Detach`].
    Requested,
    /// The Surface was destroyed (tab closed, session deleted).
    SurfaceDestroyed,
    /// The server is going away.
    ServerShutdown,
}

/// A per-message error (§9, grilling Q48). Never fatal to the connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataError {
    /// The Surface the failed message referred to, when it named one.
    pub surface_id: Option<SurfaceId>,
    /// One of the `DATA_ERR_*` constants.
    pub code: u16,
    /// Human-readable detail; for logs and banners, not for matching.
    pub message: String,
}

// ---------------------------------------------------------------- dispatch

/// Any data-plane message, tagged by its [`msg_type`].
///
/// This is a convenience for encoders, decoders and tests; the wire never
/// carries the discriminant itself — the frame's `msg_type` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataMsg {
    /// [`Hello`], `0x0001`.
    Hello(Hello),
    /// [`HelloAck`], `0x0002`.
    HelloAck(HelloAck),
    /// [`Reject`], `0x0003`.
    Reject(Reject),
    /// [`Attach`], `0x0010`.
    Attach(Attach),
    /// [`Detach`], `0x0011`.
    Detach(Detach),
    /// [`Input`], `0x0012`.
    Input(Input),
    /// [`Resize`], `0x0013`.
    Resize(Resize),
    /// [`FetchHistory`], `0x0014`.
    FetchHistory(FetchHistory),
    /// [`Ack`], `0x0015`.
    Ack(Ack),
    /// [`SetViewState`], `0x0016`.
    SetViewState(SetViewState),
    /// [`Snapshot`], `0x0100`.
    Snapshot(Box<Snapshot>),
    /// [`Delta`], `0x0101`.
    Delta(Box<Delta>),
    /// [`History`], `0x0102`.
    History(Box<History>),
    /// [`SurfaceExited`], `0x0103`.
    SurfaceExited(SurfaceExited),
    /// [`Bell`], `0x0106`.
    Bell(Bell),
    /// [`Detached`], `0x0107`.
    Detached(Detached),
    /// [`DataError`], `0x01FF`.
    DataError(DataError),
}

/// Something went wrong encoding or decoding a data-plane message.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The payload could not be decoded as the struct the `msg_type` selects,
    /// or a value could not be encoded.
    #[error("postcard codec error: {0}")]
    Postcard(#[from] postcard::Error),
    /// The frame's `msg_type` is not one this version knows.
    #[error("unknown msg_type 0x{0:04X}")]
    UnknownMsgType(u16),
    /// The encoded message does not fit in a frame.
    #[error(transparent)]
    Frame(#[from] FrameError),
}

impl DataMsg {
    /// The `msg_type` this message travels under.
    #[must_use]
    pub const fn msg_type(&self) -> u16 {
        match self {
            DataMsg::Hello(_) => msg_type::HELLO,
            DataMsg::HelloAck(_) => msg_type::HELLO_ACK,
            DataMsg::Reject(_) => msg_type::REJECT,
            DataMsg::Attach(_) => msg_type::ATTACH,
            DataMsg::Detach(_) => msg_type::DETACH,
            DataMsg::Input(_) => msg_type::INPUT,
            DataMsg::Resize(_) => msg_type::RESIZE,
            DataMsg::FetchHistory(_) => msg_type::FETCH_HISTORY,
            DataMsg::Ack(_) => msg_type::ACK,
            DataMsg::SetViewState(_) => msg_type::SET_VIEW_STATE,
            DataMsg::Snapshot(_) => msg_type::SNAPSHOT,
            DataMsg::Delta(_) => msg_type::DELTA,
            DataMsg::History(_) => msg_type::HISTORY,
            DataMsg::SurfaceExited(_) => msg_type::SURFACE_EXITED,
            DataMsg::Bell(_) => msg_type::BELL,
            DataMsg::Detached(_) => msg_type::DETACHED,
            DataMsg::DataError(_) => msg_type::DATA_ERROR,
        }
    }

    /// `true` for the messages only a client sends (§4.1).
    #[must_use]
    pub const fn is_client_to_server(&self) -> bool {
        self.msg_type() < 0x0100
    }

    /// Postcard-encodes the body (without the frame header).
    pub fn to_payload(&self) -> Result<Vec<u8>, CodecError> {
        let bytes = match self {
            DataMsg::Hello(m) => postcard::to_stdvec(m)?,
            DataMsg::HelloAck(m) => postcard::to_stdvec(m)?,
            DataMsg::Reject(m) => postcard::to_stdvec(m)?,
            DataMsg::Attach(m) => postcard::to_stdvec(m)?,
            DataMsg::Detach(m) => postcard::to_stdvec(m)?,
            DataMsg::Input(m) => postcard::to_stdvec(m)?,
            DataMsg::Resize(m) => postcard::to_stdvec(m)?,
            DataMsg::FetchHistory(m) => postcard::to_stdvec(m)?,
            DataMsg::Ack(m) => postcard::to_stdvec(m)?,
            DataMsg::SetViewState(m) => postcard::to_stdvec(m)?,
            DataMsg::Snapshot(m) => postcard::to_stdvec(m)?,
            DataMsg::Delta(m) => postcard::to_stdvec(m)?,
            DataMsg::History(m) => postcard::to_stdvec(m)?,
            DataMsg::SurfaceExited(m) => postcard::to_stdvec(m)?,
            DataMsg::Bell(m) => postcard::to_stdvec(m)?,
            DataMsg::Detached(m) => postcard::to_stdvec(m)?,
            DataMsg::DataError(m) => postcard::to_stdvec(m)?,
        };
        Ok(bytes)
    }

    /// Decodes a message given the frame's `msg_type` and payload.
    pub fn from_frame(msg_type: u16, payload: &[u8]) -> Result<Self, CodecError> {
        use self::msg_type as t;
        let msg = match msg_type {
            t::HELLO => DataMsg::Hello(postcard::from_bytes(payload)?),
            t::HELLO_ACK => DataMsg::HelloAck(postcard::from_bytes(payload)?),
            t::REJECT => DataMsg::Reject(postcard::from_bytes(payload)?),
            t::ATTACH => DataMsg::Attach(postcard::from_bytes(payload)?),
            t::DETACH => DataMsg::Detach(postcard::from_bytes(payload)?),
            t::INPUT => DataMsg::Input(postcard::from_bytes(payload)?),
            t::RESIZE => DataMsg::Resize(postcard::from_bytes(payload)?),
            t::FETCH_HISTORY => DataMsg::FetchHistory(postcard::from_bytes(payload)?),
            t::ACK => DataMsg::Ack(postcard::from_bytes(payload)?),
            t::SET_VIEW_STATE => DataMsg::SetViewState(postcard::from_bytes(payload)?),
            t::SNAPSHOT => DataMsg::Snapshot(Box::new(postcard::from_bytes(payload)?)),
            t::DELTA => DataMsg::Delta(Box::new(postcard::from_bytes(payload)?)),
            t::HISTORY => DataMsg::History(Box::new(postcard::from_bytes(payload)?)),
            t::SURFACE_EXITED => DataMsg::SurfaceExited(postcard::from_bytes(payload)?),
            t::BELL => DataMsg::Bell(postcard::from_bytes(payload)?),
            t::DETACHED => DataMsg::Detached(postcard::from_bytes(payload)?),
            t::DATA_ERROR => DataMsg::DataError(postcard::from_bytes(payload)?),
            other => return Err(CodecError::UnknownMsgType(other)),
        };
        Ok(msg)
    }

    /// Appends this message to `out` as one complete frame.
    pub fn encode_to(&self, out: &mut Vec<u8>) -> Result<(), CodecError> {
        let payload = self.to_payload()?;
        encode_frame(self.msg_type(), &payload, out)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Attrs, CellFlags, Color, PackedCell};
    use crate::control::{AbsPoint, Selection, SelectionKind};
    use crate::frame::{ClientKind, FrameDecoder, PROTO_VERSION};

    fn sample_row() -> Row {
        Row {
            cells: vec![
                PackedCell::from_char('h', StyleIdx::ZERO),
                PackedCell::new(0, StyleIdx::new(3), CellFlags::GRAPHEME_EXT),
                PackedCell::new('世' as u32, StyleIdx::new(1), CellFlags::WIDE),
                PackedCell::new(0, StyleIdx::new(1), CellFlags::WIDE_SPACER),
            ],
            extras: vec!["a".into(), "b".into(), "c".into(), "👩‍👩‍👧".into()],
            wrapped: true,
        }
    }

    fn sample_view_state() -> ViewState {
        ViewState {
            scroll_offset: 12,
            selection: Some(Selection {
                kind: SelectionKind::Block,
                anchor: AbsPoint {
                    line: AbsLine(10342),
                    col: 0,
                },
                head: AbsPoint {
                    line: AbsLine(10343),
                    col: 17,
                },
            }),
        }
    }

    pub(crate) fn every_message() -> Vec<DataMsg> {
        vec![
            DataMsg::Hello(Hello {
                proto_version: PROTO_VERSION,
                client_kind: ClientKind::Data,
                build_id: "deadbeef".into(),
            }),
            DataMsg::HelloAck(HelloAck {
                proto_version: PROTO_VERSION,
                server_build_id: "cafe-dirty".into(),
                workspace_revision: 42,
                server_pid: 4242,
            }),
            DataMsg::Reject(Reject {
                reason: crate::frame::RejectReason::NotHello,
                message: "expected Hello".into(),
                server_version: PROTO_VERSION,
            }),
            DataMsg::Attach(Attach {
                surface_id: SurfaceId(9),
                mode: AttachMode::Passive,
                want_snapshot: true,
                known_seq: Seq(0),
            }),
            DataMsg::Detach(Detach {
                surface_id: SurfaceId(9),
            }),
            DataMsg::Input(Input {
                surface_id: SurfaceId(9),
                bytes: b"\x1b[200~pasted\x1b[201~".to_vec(),
            }),
            DataMsg::Resize(Resize {
                surface_id: SurfaceId(9),
                cols: 200,
                rows: 60,
            }),
            DataMsg::FetchHistory(FetchHistory {
                surface_id: SurfaceId(9),
                from_line: AbsLine(1000),
                count: MAX_HISTORY_COUNT,
            }),
            DataMsg::Ack(Ack {
                surface_id: SurfaceId(9),
                seq: Seq(77),
            }),
            DataMsg::Snapshot(Box::new(Snapshot {
                surface_id: SurfaceId(9),
                seq: Seq(77),
                cols: 200,
                rows: 2,
                styles: vec![
                    Style::DEFAULT,
                    Style {
                        fg: Color::Indexed(9),
                        bg: Color::Rgb(1, 2, 3),
                        underline_color: Color::Default,
                        attrs: Attrs::BOLD | Attrs::UNDERLINE | Attrs::UL_CURLY,
                    },
                ],
                grid: vec![sample_row(), Row::new()],
                cursor: Cursor::default(),
                modes: Modes::ALT_SCREEN | Modes::MOUSE_SGR,
                title: "~/projects/superterminal".into(),
                history_base: AbsLine(500),
                history_len: 9_500,
                view_state: sample_view_state(),
                exited: Some(ExitStatus {
                    code: Some(0),
                    signal: None,
                }),
            })),
            DataMsg::Delta(Box::new(Delta {
                surface_id: SurfaceId(9),
                seq: Seq(78),
                since_seq: Seq(77),
                history_base: AbsLine(501),
                history_len: 9_500,
                resized: Some((100, 40)),
                new_styles: vec![(
                    StyleIdx::new(4095),
                    Style {
                        attrs: Attrs::BLINK,
                        ..Style::DEFAULT
                    },
                )],
                rows: vec![DirtyRow {
                    index: 3,
                    row: sample_row(),
                }],
                cursor: Cursor {
                    row: 3,
                    col: 4,
                    shape: CursorShape::Beam,
                    visible: false,
                    blink: false,
                },
                modes: Modes::empty(),
                title: Some("vim".into()),
            })),
            DataMsg::History(Box::new(History {
                surface_id: SurfaceId(9),
                from_line: AbsLine(500),
                history_base: AbsLine(500),
                rows: vec![sample_row(), Row::new()],
            })),
            DataMsg::SurfaceExited(SurfaceExited {
                surface_id: SurfaceId(9),
                seq: Seq(79),
                status: ExitStatus {
                    code: None,
                    signal: Some(9),
                },
            }),
            DataMsg::Bell(Bell {
                surface_id: SurfaceId(9),
            }),
            DataMsg::Detached(Detached {
                surface_id: SurfaceId(9),
                reason: DetachReason::ServerShutdown,
            }),
            DataMsg::DataError(DataError {
                surface_id: Some(SurfaceId(9)),
                code: DATA_ERR_SURFACE_EXITED,
                message: "surface_exited".into(),
            }),
        ]
    }

    #[test]
    fn msg_type_table_matches_the_spec() {
        let types: Vec<u16> = every_message().iter().map(DataMsg::msg_type).collect();
        assert_eq!(
            types,
            vec![
                0x0001, 0x0002, 0x0003, 0x0010, 0x0011, 0x0012, 0x0013, 0x0014, 0x0015, 0x0100,
                0x0101, 0x0102, 0x0103, 0x0106, 0x0107, 0x01FF
            ]
        );
    }

    #[test]
    fn every_message_round_trips_through_postcard() {
        for msg in every_message() {
            let payload = msg.to_payload().unwrap();
            let back = DataMsg::from_frame(msg.msg_type(), &payload).unwrap();
            assert_eq!(back, msg, "round trip failed for {msg:?}");
        }
    }

    #[test]
    fn every_message_round_trips_through_a_frame() {
        let mut wire = Vec::new();
        let msgs = every_message();
        for msg in &msgs {
            msg.encode_to(&mut wire).unwrap();
        }
        let mut dec = FrameDecoder::new();
        dec.push(&wire);
        for expected in &msgs {
            let frame = dec.next_frame().unwrap().unwrap();
            let got = DataMsg::from_frame(frame.msg_type, &frame.payload).unwrap();
            assert_eq!(&got, expected);
        }
        assert_eq!(dec.next_frame().unwrap(), None);
    }

    #[test]
    fn unknown_msg_type_is_an_error() {
        let err = DataMsg::from_frame(0x0104, &[]).unwrap_err();
        assert!(matches!(err, CodecError::UnknownMsgType(0x0104)));
        assert_eq!(err.to_string(), "unknown msg_type 0x0104");
    }

    #[test]
    fn direction_split_at_0x0100() {
        assert!(!DataMsg::Bell(Bell {
            surface_id: SurfaceId(1)
        })
        .is_client_to_server());
        assert!(DataMsg::Detach(Detach {
            surface_id: SurfaceId(1)
        })
        .is_client_to_server());
    }

    #[test]
    fn modes_helpers_and_wire_width() {
        assert!(Modes::MOUSE_DRAG.mouse_reporting());
        assert!(!Modes::MOUSE_SGR.mouse_reporting());
        let bytes = postcard::to_stdvec(&(Modes::ALT_SCREEN | Modes::LINE_WRAP)).unwrap();
        assert_eq!(
            postcard::from_bytes::<Modes>(&bytes).unwrap(),
            Modes::ALT_SCREEN | Modes::LINE_WRAP
        );
    }

    #[test]
    fn first_visible_line_derivation() {
        let DataMsg::Snapshot(snap) = &every_message()[9] else {
            panic!("expected a snapshot");
        };
        assert_eq!(snap.first_visible_line(), AbsLine(10_000));
        let DataMsg::Delta(delta) = &every_message()[10] else {
            panic!("expected a delta");
        };
        assert_eq!(delta.first_visible_line(), AbsLine(10_001));
    }

    #[test]
    fn a_typical_delta_is_small() {
        // §11: one dirty 40-column ASCII row should cost a few hundred bytes.
        let row = Row {
            cells: (0..40)
                .map(|i| PackedCell::from_char((b'a' + (i % 26) as u8) as char, StyleIdx::ZERO))
                .collect(),
            extras: Vec::new(),
            wrapped: false,
        };
        let delta = Delta {
            surface_id: SurfaceId(9),
            seq: Seq(2),
            since_seq: Seq(1),
            history_base: AbsLine(0),
            history_len: 1,
            resized: None,
            new_styles: Vec::new(),
            rows: vec![DirtyRow { index: 59, row }],
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: None,
        };
        let len = postcard::to_stdvec(&delta).unwrap().len();
        assert!(len < 300, "delta was {len} bytes");
    }
}
