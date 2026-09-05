//! Transport framing and the handshake — `docs/plan/02-protocol.md` §1–§2.
//!
//! Every connection is CONTROL or DATA for its whole lifetime and the server
//! classifies it by the **first byte**:
//!
//! * `0x7B` (`{`) → CONTROL: newline-delimited JSON, at most
//!   [`MAX_CONTROL_LINE`] bytes per line.
//! * `0xFF` → DATA: the 4-byte magic [`DATA_MAGIC`] (`0xFF "STD"`), then
//!   binary frames.
//!
//! A DATA frame is
//!
//! ```text
//! +----------------+----------------+------------------------+
//! | u32 len  (LE)  | u16 msg_type   | payload (postcard)     |
//! +----------------+----------------+------------------------+
//!   len = 2 + payload.len()   (the header's own 4 bytes excluded)
//! ```
//!
//! Frames are never interleaved; the two directions are independent streams.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// First byte of a CONTROL connection: `{`, the start of a JSON object.
pub const CONTROL_FIRST_BYTE: u8 = b'{';

/// First bytes of a DATA connection: `0xFF "STD"` (grilling Q37).
///
/// `0xFF` never occurs in valid UTF-8, so the two connection kinds can never
/// be confused.
pub const DATA_MAGIC: [u8; 4] = [0xFF, b'S', b'T', b'D'];

/// Size of a DATA frame header: `u32 len` + `u16 msg_type`.
pub const FRAME_HEADER_LEN: usize = 6;

/// Hard cap on `len` (`msg_type` + payload) of a single DATA frame.
///
/// A frame larger than this closes the connection. This is a sanity bound, not
/// a design limit: the worst-case 200×60 Snapshot in §11 is ≈220 KB.
///
/// **Deviation:** `02-protocol.md` §1.3 writes 8 MiB; this crate enforces the
/// 16 MiB cap specified for the implementation. Anything a v1 server produces
/// is far below either number.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Largest payload that fits in a frame, i.e. [`MAX_FRAME`] minus `msg_type`.
pub const MAX_PAYLOAD: usize = MAX_FRAME - 2;

/// Maximum length of one CONTROL line, excluding the terminating `\n` (§1.2).
pub const MAX_CONTROL_LINE: usize = 4 * 1024 * 1024;

/// Handshake timeout: a connection that has not sent `Hello` within this many
/// seconds is closed (§2, rule 1).
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 5;

/// Which plane a connection speaks, decided by its first byte (§1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionKind {
    /// Newline-delimited JSON control plane.
    Control,
    /// Binary, postcard-framed data plane.
    Data,
}

/// Classifies a connection from its first byte. `None` means the server must
/// close the connection.
#[must_use]
pub fn detect_connection_kind(first_byte: u8) -> Option<ConnectionKind> {
    match first_byte {
        CONTROL_FIRST_BYTE => Some(ConnectionKind::Control),
        b if b == DATA_MAGIC[0] => Some(ConnectionKind::Data),
        _ => None,
    }
}

/// Protocol version. On the wire this is `u16 = major << 8 | minor` on the
/// data plane and the string `"major.minor"` on the control plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProtoVersion {
    /// Breaking version. Must match exactly between client and server.
    pub major: u8,
    /// Additive version. The negotiated minor is `min(client, server)`.
    pub minor: u8,
}

/// The version implemented by this crate.
/// 1.1 added `Tab.layout`, `tab.split`, `pane.close` and `tab.set_ratio`
/// (ADR 0009); a 1.0 peer negotiates down and sees only the first Pane.
pub const PROTO_VERSION: ProtoVersion = ProtoVersion { major: 1, minor: 1 };

impl ProtoVersion {
    /// Builds a version from its parts.
    #[inline]
    #[must_use]
    pub const fn new(major: u8, minor: u8) -> Self {
        Self { major, minor }
    }

    /// Packs the version as `major << 8 | minor`.
    #[inline]
    #[must_use]
    pub const fn to_u16(self) -> u16 {
        ((self.major as u16) << 8) | self.minor as u16
    }

    /// Unpacks a version from `major << 8 | minor`.
    #[inline]
    #[must_use]
    pub const fn from_u16(bits: u16) -> Self {
        Self {
            major: (bits >> 8) as u8,
            minor: bits as u8,
        }
    }

    /// Returns `true` when the two sides can talk at all: the major versions
    /// are equal (§2, rule 2).
    #[inline]
    #[must_use]
    pub const fn compatible_with(self, other: Self) -> bool {
        self.major == other.major
    }

    /// The version both sides must restrict themselves to: same major, the
    /// lower minor (§2, rule 3). `None` on a major mismatch.
    #[inline]
    #[must_use]
    pub const fn negotiate(self, other: Self) -> Option<Self> {
        if self.major != other.major {
            return None;
        }
        let minor = if self.minor < other.minor {
            self.minor
        } else {
            other.minor
        };
        Some(Self {
            major: self.major,
            minor,
        })
    }
}

impl fmt::Display for ProtoVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl std::str::FromStr for ProtoVersion {
    type Err = ParseVersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (major, minor) = s.split_once('.').ok_or(ParseVersionError)?;
        Ok(Self {
            major: major.parse().map_err(|_| ParseVersionError)?,
            minor: minor.parse().map_err(|_| ParseVersionError)?,
        })
    }
}

/// A [`ProtoVersion`] string was not of the form `"major.minor"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("expected a protocol version of the form \"major.minor\"")]
pub struct ParseVersionError;

impl Serialize for ProtoVersion {
    /// `"1.0"` on a self-describing format (CONTROL/JSON), `u16` otherwise
    /// (DATA/postcard).
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.collect_str(self)
        } else {
            serializer.serialize_u16(self.to_u16())
        }
    }
}

impl<'de> Deserialize<'de> for ProtoVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StrVisitor;
        impl Visitor<'_> for StrVisitor {
            type Value = ProtoVersion;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a protocol version string like \"1.0\"")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                v.parse().map_err(E::custom)
            }
        }

        if deserializer.is_human_readable() {
            deserializer.deserialize_str(StrVisitor)
        } else {
            Ok(ProtoVersion::from_u16(u16::deserialize(deserializer)?))
        }
    }
}

/// What a connecting peer is (§2). `Tool` is the CLI/inspector: control-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    /// The Bun app's control-plane connection.
    Control,
    /// The native client's data-plane connection.
    Data,
    /// `st` CLI or inspector; control plane only.
    Tool,
}

/// First message on every connection, client → server (§2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The version the client implements.
    pub proto_version: ProtoVersion,
    /// What the client is.
    pub client_kind: ClientKind,
    /// git sha + dirty flag. Informational only; never used for decisions.
    pub build_id: String,
}

/// The server's answer to a compatible [`Hello`] (§2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAck {
    /// The negotiated version: same major, `min` of the minors.
    pub proto_version: ProtoVersion,
    /// The server's build id, shown in `server.status`.
    pub server_build_id: String,
    /// Current Workspace document revision.
    pub workspace_revision: u64,
    /// The daemon's pid.
    pub server_pid: u32,
}

/// The server's answer to a connection it will not serve (§2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reject {
    /// Machine-readable reason.
    pub reason: RejectReason,
    /// Human-readable text, shown in the client's banner.
    pub message: String,
    /// The version the server implements, so the banner can explain the gap.
    pub server_version: ProtoVersion,
}

/// Why a connection was rejected (§2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// `Hello.proto_version.major` differs from the server's.
    MajorMismatch,
    /// A DATA connection did not begin with [`DATA_MAGIC`].
    BadMagic,
    /// A CONTROL line exceeded [`MAX_CONTROL_LINE`].
    LineTooLong,
    /// A DATA frame exceeded [`MAX_FRAME`].
    FrameTooLarge,
    /// The first message was not a `Hello`.
    NotHello,
    /// The server is shutting down and accepts no new connections.
    ShuttingDown,
}

/// A decoded DATA frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The `msg_type` from the header; see [`crate::data::msg_type`].
    pub msg_type: u16,
    /// The postcard-encoded body.
    pub payload: Vec<u8>,
}

/// Framing errors. Every one of these is fatal for the connection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// A frame header announced a `len` above [`MAX_FRAME`].
    #[error("frame length {len} exceeds the {max} byte maximum")]
    FrameTooLarge {
        /// The announced `len` (`msg_type` + payload).
        len: usize,
        /// The configured maximum.
        max: usize,
    },
    /// A frame header announced `len < 2`, which cannot even hold `msg_type`.
    #[error("frame length {len} is shorter than the 2-byte msg_type")]
    FrameTooShort {
        /// The announced `len`.
        len: usize,
    },
    /// The payload handed to [`encode_frame`] does not fit in a frame.
    #[error("payload of {len} bytes exceeds the {max} byte maximum")]
    PayloadTooLarge {
        /// The payload length.
        len: usize,
        /// The largest payload that fits.
        max: usize,
    },
    /// A DATA connection did not begin with [`DATA_MAGIC`].
    #[error("expected the DATA magic 0xFF \"STD\"")]
    BadMagic,
    /// The decoder hit a fatal error earlier; the connection must be closed.
    #[error("frame decoder is in a failed state")]
    Poisoned,
}

impl FrameError {
    /// The [`RejectReason`] to report for this error, when the connection is
    /// still healthy enough to send a `Reject`.
    #[must_use]
    pub const fn reject_reason(&self) -> RejectReason {
        match self {
            FrameError::BadMagic => RejectReason::BadMagic,
            _ => RejectReason::FrameTooLarge,
        }
    }
}

/// Appends one framed message to `out`.
///
/// Writes `u32 len` (little-endian, `= 2 + payload.len()`), `u16 msg_type`
/// (little-endian) and the payload. Fails when the payload cannot fit under
/// [`MAX_FRAME`].
pub fn encode_frame(msg_type: u16, payload: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(FrameError::PayloadTooLarge {
            len: payload.len(),
            max: MAX_PAYLOAD,
        });
    }
    let len = (payload.len() + 2) as u32;
    out.reserve(FRAME_HEADER_LEN + payload.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&msg_type.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

/// Encodes one frame into a fresh `Vec`.
pub fn encode_frame_to_vec(msg_type: u16, payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    let mut out = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
    encode_frame(msg_type, payload, &mut out)?;
    Ok(out)
}

/// Byte-stream → frames.
///
/// Feed arbitrary chunks with [`push`](FrameDecoder::push) and drain complete
/// frames with [`next_frame`](FrameDecoder::next_frame); the decoder holds a
/// partial frame across calls. It is transport-agnostic: it never reads or
/// writes a socket itself.
///
/// After any error the decoder is *poisoned* and keeps returning
/// [`FrameError::Poisoned`]: framing errors are unrecoverable, the connection
/// must be closed.
#[derive(Debug)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    pos: usize,
    max_frame: usize,
    awaiting_magic: bool,
    poisoned: bool,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Compact the buffer once this many consumed bytes have piled up in front.
const COMPACT_THRESHOLD: usize = 64 * 1024;

impl FrameDecoder {
    /// A decoder for a stream that starts directly with a frame header (the
    /// magic has already been consumed, or this is the server → client
    /// direction, which carries no magic).
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            pos: 0,
            max_frame: MAX_FRAME,
            awaiting_magic: false,
            poisoned: false,
        }
    }

    /// A decoder that first expects the 4-byte [`DATA_MAGIC`] — what the
    /// server uses for an incoming DATA connection.
    #[must_use]
    pub fn expecting_magic() -> Self {
        Self {
            awaiting_magic: true,
            ..Self::new()
        }
    }

    /// Overrides the maximum frame length (tests, tooling). Values above
    /// [`MAX_FRAME`] are clamped.
    #[must_use]
    pub fn with_max_frame(mut self, max_frame: usize) -> Self {
        self.max_frame = max_frame.min(MAX_FRAME);
        self
    }

    /// The maximum frame length this decoder enforces.
    #[inline]
    #[must_use]
    pub const fn max_frame(&self) -> usize {
        self.max_frame
    }

    /// Number of buffered bytes not yet formed into a frame.
    #[inline]
    #[must_use]
    pub fn buffered_len(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// `true` once a framing error has been returned.
    #[inline]
    #[must_use]
    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Appends freshly read bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.pos > COMPACT_THRESHOLD {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// Returns the next complete frame, or `Ok(None)` when more bytes are
    /// needed.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        if self.poisoned {
            return Err(FrameError::Poisoned);
        }
        match self.decode() {
            Err(err) => {
                self.poisoned = true;
                Err(err)
            }
            ok => ok,
        }
    }

    fn decode(&mut self) -> Result<Option<Frame>, FrameError> {
        if self.awaiting_magic {
            if self.buffered_len() < DATA_MAGIC.len() {
                return Ok(None);
            }
            let head = &self.buf[self.pos..self.pos + DATA_MAGIC.len()];
            if head != DATA_MAGIC {
                return Err(FrameError::BadMagic);
            }
            self.pos += DATA_MAGIC.len();
            self.awaiting_magic = false;
        }

        if self.buffered_len() < FRAME_HEADER_LEN {
            return Ok(None);
        }
        let header = &self.buf[self.pos..self.pos + FRAME_HEADER_LEN];
        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        if len < 2 {
            return Err(FrameError::FrameTooShort { len });
        }
        if len > self.max_frame {
            return Err(FrameError::FrameTooLarge {
                len,
                max: self.max_frame,
            });
        }
        let msg_type = u16::from_le_bytes([header[4], header[5]]);
        let payload_len = len - 2;
        if self.buffered_len() < FRAME_HEADER_LEN + payload_len {
            self.buf
                .reserve(FRAME_HEADER_LEN + payload_len - self.buffered_len());
            return Ok(None);
        }
        let start = self.pos + FRAME_HEADER_LEN;
        let payload = self.buf[start..start + payload_len].to_vec();
        self.pos = start + payload_len;
        if self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        }
        Ok(Some(Frame { msg_type, payload }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_kind_sniffing() {
        assert_eq!(detect_connection_kind(b'{'), Some(ConnectionKind::Control));
        assert_eq!(detect_connection_kind(0xFF), Some(ConnectionKind::Data));
        assert_eq!(detect_connection_kind(b'h'), None);
        assert_eq!(detect_connection_kind(0), None);
        assert_eq!(DATA_MAGIC, [0xFF, b'S', b'T', b'D']);
    }

    #[test]
    fn frame_header_layout() {
        let out = encode_frame_to_vec(0x0101, &[1, 2, 3]).unwrap();
        assert_eq!(out, vec![5, 0, 0, 0, 0x01, 0x01, 1, 2, 3]);
    }

    #[test]
    fn round_trip_one_frame() {
        let mut dec = FrameDecoder::new();
        dec.push(&encode_frame_to_vec(0x0012, b"hello").unwrap());
        let frame = dec.next_frame().unwrap().unwrap();
        assert_eq!(frame.msg_type, 0x0012);
        assert_eq!(frame.payload, b"hello");
        assert_eq!(dec.next_frame().unwrap(), None);
        assert_eq!(dec.buffered_len(), 0);
    }

    #[test]
    fn empty_payload_is_legal() {
        let mut dec = FrameDecoder::new();
        dec.push(&encode_frame_to_vec(0x0106, &[]).unwrap());
        let frame = dec.next_frame().unwrap().unwrap();
        assert_eq!(frame.msg_type, 0x0106);
        assert!(frame.payload.is_empty());
    }

    #[test]
    fn magic_is_consumed_before_frames() {
        let mut dec = FrameDecoder::expecting_magic();
        dec.push(&DATA_MAGIC);
        assert_eq!(dec.next_frame().unwrap(), None);
        dec.push(&encode_frame_to_vec(1, b"x").unwrap());
        assert_eq!(dec.next_frame().unwrap().unwrap().msg_type, 1);
    }

    #[test]
    fn bad_magic_poisons_the_decoder() {
        let mut dec = FrameDecoder::expecting_magic();
        dec.push(b"{\"t\"");
        assert_eq!(dec.next_frame(), Err(FrameError::BadMagic));
        assert!(dec.is_poisoned());
        assert_eq!(dec.next_frame(), Err(FrameError::Poisoned));
    }

    #[test]
    fn oversized_frame_is_rejected_without_buffering_it() {
        let mut dec = FrameDecoder::new();
        let len = (MAX_FRAME + 1) as u32;
        dec.push(&len.to_le_bytes());
        dec.push(&0x0100u16.to_le_bytes());
        assert_eq!(
            dec.next_frame(),
            Err(FrameError::FrameTooLarge {
                len: MAX_FRAME + 1,
                max: MAX_FRAME
            })
        );
        assert_eq!(
            FrameError::FrameTooLarge {
                len: 0,
                max: MAX_FRAME
            }
            .reject_reason(),
            RejectReason::FrameTooLarge
        );
    }

    #[test]
    fn frame_at_exactly_the_cap_is_accepted() {
        let mut dec = FrameDecoder::new().with_max_frame(1024);
        let payload = vec![7u8; 1022];
        dec.push(&encode_frame_to_vec(0x0100, &payload).unwrap());
        assert_eq!(dec.next_frame().unwrap().unwrap().payload, payload);

        let mut dec = FrameDecoder::new().with_max_frame(1024);
        dec.push(&encode_frame_to_vec(0x0100, &vec![7u8; 1023]).unwrap());
        assert!(matches!(
            dec.next_frame(),
            Err(FrameError::FrameTooLarge { .. })
        ));
    }

    #[test]
    fn truncated_length_is_rejected() {
        let mut dec = FrameDecoder::new();
        dec.push(&[1, 0, 0, 0, 0, 0]);
        assert_eq!(dec.next_frame(), Err(FrameError::FrameTooShort { len: 1 }));
    }

    #[test]
    fn oversized_payload_cannot_be_encoded() {
        let mut out = Vec::new();
        let err = encode_frame(1, &vec![0u8; MAX_PAYLOAD + 1], &mut out).unwrap_err();
        assert!(matches!(err, FrameError::PayloadTooLarge { .. }));
        assert!(out.is_empty());
    }

    #[test]
    fn byte_by_byte_feeding() {
        let mut wire = Vec::new();
        encode_frame(0x0100, b"snapshot", &mut wire).unwrap();
        encode_frame(0x0101, b"delta", &mut wire).unwrap();

        let mut dec = FrameDecoder::new();
        let mut got = Vec::new();
        for byte in &wire {
            dec.push(std::slice::from_ref(byte));
            while let Some(frame) = dec.next_frame().unwrap() {
                got.push(frame);
            }
        }
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].payload, b"snapshot");
        assert_eq!(got[1].msg_type, 0x0101);
    }

    #[test]
    fn many_frames_in_one_push_and_buffer_compaction() {
        let mut wire = Vec::new();
        for i in 0..500u16 {
            encode_frame(i, &vec![(i % 251) as u8; 400], &mut wire).unwrap();
        }
        let mut dec = FrameDecoder::new();
        dec.push(&wire);
        for i in 0..500u16 {
            let frame = dec.next_frame().unwrap().unwrap();
            assert_eq!(frame.msg_type, i);
            assert_eq!(frame.payload.len(), 400);
        }
        assert_eq!(dec.next_frame().unwrap(), None);
        assert_eq!(dec.buffered_len(), 0);
    }

    #[test]
    fn version_negotiation() {
        assert_eq!(PROTO_VERSION.to_string(), "1.1");
        assert_eq!(PROTO_VERSION.to_u16(), 0x0101);
        assert_eq!(ProtoVersion::from_u16(0x0101), PROTO_VERSION);
        assert_eq!(
            PROTO_VERSION.negotiate(ProtoVersion::new(1, 4)),
            Some(ProtoVersion::new(1, 1))
        );
        assert_eq!(
            PROTO_VERSION.negotiate(ProtoVersion::new(1, 0)),
            Some(ProtoVersion::new(1, 0)),
            "a 1.0 peer negotiates down"
        );
        assert_eq!(
            ProtoVersion::new(1, 7).negotiate(ProtoVersion::new(1, 4)),
            Some(ProtoVersion::new(1, 4))
        );
        assert_eq!(PROTO_VERSION.negotiate(ProtoVersion::new(2, 0)), None);
        assert!(!PROTO_VERSION.compatible_with(ProtoVersion::new(2, 0)));
    }

    #[test]
    fn version_is_a_string_in_json_and_a_u16_in_postcard() {
        assert_eq!(serde_json::to_string(&PROTO_VERSION).unwrap(), "\"1.1\"");
        assert_eq!(
            serde_json::from_str::<ProtoVersion>("\"1.1\"").unwrap(),
            PROTO_VERSION
        );
        assert!(serde_json::from_str::<ProtoVersion>("\"x\"").is_err());
        let bytes = postcard::to_stdvec(&PROTO_VERSION).unwrap();
        assert_eq!(
            postcard::from_bytes::<ProtoVersion>(&bytes).unwrap(),
            PROTO_VERSION
        );
        assert_eq!("1.1".parse::<ProtoVersion>().unwrap(), PROTO_VERSION);
        assert!("1".parse::<ProtoVersion>().is_err());
    }

    #[test]
    fn handshake_json_shapes() {
        let hello = Hello {
            proto_version: PROTO_VERSION,
            client_kind: ClientKind::Control,
            build_id: "abc123-dirty".into(),
        };
        assert_eq!(
            serde_json::to_value(&hello).unwrap(),
            serde_json::json!({
                "proto_version": "1.1",
                "client_kind": "control",
                "build_id": "abc123-dirty",
            })
        );
        let reject = Reject {
            reason: RejectReason::MajorMismatch,
            message: "server speaks 1.x".into(),
            server_version: PROTO_VERSION,
        };
        assert_eq!(
            serde_json::to_value(&reject).unwrap()["reason"],
            serde_json::json!("major_mismatch")
        );
    }
}
