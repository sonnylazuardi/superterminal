//! The DATA plane client — `docs/plan/02-protocol.md` §1.3, §2, §4.
//!
//! Opening sequence, in order:
//!
//! 1. the 4-byte magic `0xFF "STD"` ([`st_proto::DATA_MAGIC`], grilling Q37),
//! 2. a `Hello` frame (`msg_type` `0x0001`),
//! 3. the server's `HelloAck` (`0x0002`) or `Reject` (`0x0003`),
//! 4. an `Attach` frame (`0x0010`) for the Surface we want.
//!
//! Only then does the server start sending `Snapshot`/`Delta`.
//!
//! **Note on `client_kind`:** §2 calls `Tool` "control-only", so a DATA
//! connection announces [`ClientKind::Data`] even though the peer is `st`.
//! `build_id` says which tool it actually is, and §2 rule 4 makes `build_id`
//! informational, so nothing depends on the distinction.

use std::io::{Read, Write};

use st_proto::{
    Ack, Attach, AttachMode, ClientKind, DataMsg, FrameDecoder, Hello, HelloAck, Seq, SurfaceId,
    DATA_MAGIC, PROTO_VERSION,
};

use crate::exit::CliError;
use crate::transport::{Connector, Transport};

/// A connected, handshaken DATA client.
pub struct DataClient {
    stream: Box<dyn Transport>,
    decoder: FrameDecoder,
    read_buf: Vec<u8>,
    ack: HelloAck,
    eof: bool,
}

impl DataClient {
    /// Connects and completes the magic + `Hello`/`HelloAck` handshake.
    pub fn connect(connector: &dyn Connector) -> Result<Self, CliError> {
        Self::handshake(connector.connect()?)
    }

    /// Completes the handshake on an already-open stream.
    pub fn handshake(stream: Box<dyn Transport>) -> Result<Self, CliError> {
        let mut client = Self {
            stream,
            decoder: FrameDecoder::new(),
            read_buf: vec![0u8; 64 * 1024],
            ack: HelloAck {
                proto_version: PROTO_VERSION,
                server_build_id: String::new(),
                workspace_revision: 0,
                server_pid: 0,
            },
            eof: false,
        };

        client
            .stream
            .write_all(&DATA_MAGIC)
            .map_err(|e| CliError::no_server(format!("cannot send the DATA magic: {e}")))?;
        client.send(&DataMsg::Hello(Hello {
            proto_version: PROTO_VERSION,
            client_kind: ClientKind::Data,
            build_id: crate::build_id(),
        }))?;

        match client.recv()? {
            Some(DataMsg::HelloAck(ack)) => {
                if !ack.proto_version.compatible_with(PROTO_VERSION) {
                    return Err(CliError::protocol(format!(
                        "protocol major mismatch: server speaks {}, this st speaks {PROTO_VERSION}",
                        ack.proto_version
                    )));
                }
                client.ack = ack;
                Ok(client)
            }
            Some(DataMsg::Reject(reject)) => Err(CliError::protocol(format!(
                "server rejected the data connection ({:?}): {}",
                reject.reason, reject.message
            ))),
            Some(other) => Err(CliError::protocol(format!(
                "expected HelloAck, got msg_type 0x{:04X}",
                other.msg_type()
            ))),
            None => Err(CliError::no_server(
                "the server closed the data connection during the handshake",
            )),
        }
    }

    /// The server's `HelloAck`.
    #[must_use]
    pub fn hello_ack(&self) -> &HelloAck {
        &self.ack
    }

    /// Sends `Attach{surface_id, mode, want_snapshot: true, known_seq: 0}`
    /// (§4.2). `want_snapshot` is always true: `st probe` starts from nothing.
    pub fn attach(&mut self, surface_id: SurfaceId, mode: AttachMode) -> Result<(), CliError> {
        self.send(&DataMsg::Attach(Attach {
            surface_id,
            mode,
            want_snapshot: true,
            known_seq: Seq(0),
        }))
    }

    /// Sends `Ack{surface_id, seq}`, reopening the server's in-flight window
    /// (§6.5).
    pub fn ack(&mut self, surface_id: SurfaceId, seq: Seq) -> Result<(), CliError> {
        self.send(&DataMsg::Ack(Ack { surface_id, seq }))
    }

    /// Encodes and writes one message as a single frame.
    pub fn send(&mut self, msg: &DataMsg) -> Result<(), CliError> {
        let mut wire = Vec::new();
        msg.encode_to(&mut wire)
            .map_err(|e| CliError::protocol(format!("cannot encode a data frame: {e}")))?;
        self.stream
            .write_all(&wire)
            .and_then(|()| self.stream.flush())
            .map_err(|e| CliError::no_server(format!("data connection lost while writing: {e}")))
    }

    /// Reads the next message, or `None` at end of stream.
    pub fn recv(&mut self) -> Result<Option<DataMsg>, CliError> {
        loop {
            if let Some(frame) = self
                .decoder
                .next_frame()
                .map_err(|e| CliError::protocol(format!("framing error: {e}")))?
            {
                return DataMsg::from_frame(frame.msg_type, &frame.payload)
                    .map(Some)
                    .map_err(|e| {
                        CliError::protocol(format!(
                            "cannot decode msg_type 0x{:04X}: {e}",
                            frame.msg_type
                        ))
                    });
            }
            if self.eof {
                return Ok(None);
            }
            match self.stream.read(&mut self.read_buf) {
                Ok(0) => {
                    self.eof = true;
                    if self.decoder.buffered_len() > 0 {
                        return Err(CliError::protocol(format!(
                            "the server closed the data connection mid-frame ({} bytes buffered)",
                            self.decoder.buffered_len()
                        )));
                    }
                    return Ok(None);
                }
                Ok(n) => self.decoder.push(&self.read_buf[..n]),
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(err) => {
                    return Err(CliError::no_server(format!(
                        "data connection lost while reading: {err}"
                    )))
                }
            }
        }
    }
}

/// Decodes a recorded byte stream (what `SUPERTERMINAL_RECORD=1` writes) into
/// messages, tolerating the leading [`st_proto::DATA_MAGIC`].
///
/// Returns the messages decoded so far together with the error that stopped
/// the walk, so `st dump-data` can show a truncated recording up to the point
/// where it broke.
#[must_use]
pub fn decode_recording(bytes: &[u8]) -> (Vec<RecordedFrame>, Option<CliError>) {
    let body = bytes.strip_prefix(&DATA_MAGIC[..]).unwrap_or(bytes);
    let mut decoder = FrameDecoder::new();
    decoder.push(body);

    let mut out = Vec::new();
    loop {
        match decoder.next_frame() {
            Ok(Some(frame)) => {
                let len = frame.payload.len();
                match DataMsg::from_frame(frame.msg_type, &frame.payload) {
                    Ok(msg) => out.push(RecordedFrame {
                        msg_type: frame.msg_type,
                        payload_len: len,
                        msg: Some(msg),
                    }),
                    Err(err) => {
                        out.push(RecordedFrame {
                            msg_type: frame.msg_type,
                            payload_len: len,
                            msg: None,
                        });
                        return (
                            out,
                            Some(CliError::protocol(format!(
                                "cannot decode msg_type 0x{:04X}: {err}",
                                frame.msg_type
                            ))),
                        );
                    }
                }
            }
            Ok(None) => {
                let left = decoder.buffered_len();
                let err = (left > 0).then(|| {
                    CliError::protocol(format!("{left} trailing bytes are not a complete frame"))
                });
                return (out, err);
            }
            Err(err) => {
                return (
                    out,
                    Some(CliError::protocol(format!("framing error: {err}"))),
                )
            }
        }
    }
}

/// One frame recovered from a recording.
#[derive(Debug)]
pub struct RecordedFrame {
    /// The `msg_type` from the frame header.
    pub msg_type: u16,
    /// Payload size in bytes, excluding the 6-byte header.
    pub payload_len: usize,
    /// The decoded message; `None` when the payload did not decode.
    pub msg: Option<DataMsg>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_proto::{Bell, Detach, Input};

    fn record(msgs: &[DataMsg], with_magic: bool) -> Vec<u8> {
        let mut wire = Vec::new();
        if with_magic {
            wire.extend_from_slice(&DATA_MAGIC);
        }
        for msg in msgs {
            msg.encode_to(&mut wire).unwrap();
        }
        wire
    }

    #[test]
    fn a_recording_decodes_with_or_without_the_magic() {
        let msgs = vec![
            DataMsg::Bell(Bell {
                surface_id: SurfaceId(3),
            }),
            DataMsg::Detach(Detach {
                surface_id: SurfaceId(3),
            }),
        ];
        for magic in [true, false] {
            let (frames, err) = decode_recording(&record(&msgs, magic));
            assert!(err.is_none(), "unexpected error: {err:?}");
            assert_eq!(frames.len(), 2);
            assert_eq!(frames[0].msg_type, st_proto::msg_type::BELL);
            assert_eq!(frames[1].msg_type, st_proto::msg_type::DETACH);
        }
    }

    #[test]
    fn a_truncated_recording_yields_what_it_can_plus_an_error() {
        let mut wire = record(
            &[DataMsg::Input(Input {
                surface_id: SurfaceId(1),
                bytes: b"ls\r".to_vec(),
            })],
            true,
        );
        let good = wire.len();
        wire.extend_from_slice(&[9, 0, 0, 0, 0x12]); // header of a frame that never arrives
        let (frames, err) = decode_recording(&wire);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload_len, good - DATA_MAGIC.len() - 6);
        assert!(err.unwrap().message.contains("trailing bytes"));
    }

    #[test]
    fn an_unknown_msg_type_stops_the_walk() {
        let mut wire = Vec::new();
        st_proto::encode_frame(0x0104, &[], &mut wire).unwrap();
        let (frames, err) = decode_recording(&wire);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].msg.is_none());
        assert!(err.unwrap().message.contains("0x0104"));
    }
}
