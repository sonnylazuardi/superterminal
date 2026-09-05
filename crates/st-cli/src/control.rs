//! The CONTROL plane client — `docs/plan/02-protocol.md` §1.2, §2, §3.
//!
//! Newline-delimited JSON, UTF-8, one message per `\n`-terminated line. The
//! server classifies the connection from the first byte, which is the `{` of
//! our `hello` line, so nothing else has to be sent to select the plane.
//!
//! The CLI announces itself as [`ClientKind::Tool`] ("CLI/inspector,
//! control-only", §2).

use std::io::{BufRead, BufReader, Write};

use serde_json::Value;

use st_proto::control::{AnyRes, ErrorCode, Handshake, Req, ReqId, Res};
use st_proto::{ClientKind, Ev, Hello, HelloAck, MAX_CONTROL_LINE, PROTO_VERSION};

use crate::exit::{CliError, ExitCode};
use crate::transport::{Connector, Transport};

/// A connected, handshaken CONTROL client.
pub struct ControlClient {
    reader: BufReader<Box<dyn Transport>>,
    writer: Box<dyn Transport>,
    next_id: ReqId,
    ack: HelloAck,
}

impl ControlClient {
    /// Connects through `connector` and completes the `hello`/`hello.ack`
    /// exchange (§2).
    pub fn connect(connector: &dyn Connector) -> Result<Self, CliError> {
        let stream = connector.connect()?;
        Self::handshake(stream)
    }

    /// Completes the handshake on an already-open stream.
    pub fn handshake(stream: Box<dyn Transport>) -> Result<Self, CliError> {
        let writer = stream.try_clone_box().map_err(|e| {
            CliError::failure(format!("cannot duplicate the control connection: {e}"))
        })?;
        let mut client = Self {
            reader: BufReader::new(stream),
            writer,
            next_id: 1,
            ack: HelloAck {
                proto_version: PROTO_VERSION,
                server_build_id: String::new(),
                workspace_revision: 0,
                server_pid: 0,
            },
        };

        client.send(&Handshake::Hello(Hello {
            proto_version: PROTO_VERSION,
            client_kind: ClientKind::Tool,
            build_id: crate::build_id(),
        }))?;

        let line = client.read_line()?;
        match serde_json::from_str::<Handshake>(&line) {
            Ok(Handshake::HelloAck(ack)) => {
                if !ack.proto_version.compatible_with(PROTO_VERSION) {
                    return Err(version_mismatch(ack.proto_version));
                }
                client.ack = ack;
                Ok(client)
            }
            Ok(Handshake::Reject(reject)) => Err(CliError::protocol(format!(
                "server rejected the connection ({:?}): {}",
                reject.reason, reject.message
            ))
            .with_hint(format!(
                "server speaks protocol {}, this st speaks {PROTO_VERSION}",
                reject.server_version
            ))),
            Ok(Handshake::Hello(_)) => Err(CliError::protocol(
                "server answered hello with another hello",
            )),
            Err(err) => Err(CliError::protocol(format!(
                "server sent an unparseable handshake line: {err}"
            ))
            .with_hint(truncate(&line, 200))),
        }
    }

    /// The server's `hello.ack`: build id, pid and workspace revision (§2).
    #[must_use]
    pub fn hello_ack(&self) -> &HelloAck {
        &self.ack
    }

    /// Sends one request and returns its `result` as raw JSON.
    ///
    /// Unsolicited `ev.*` lines and responses to other ids are skipped, as §3.1
    /// allows responses to arrive in any order.
    pub fn request_raw(&mut self, make: impl FnOnce(ReqId) -> Req) -> Result<Value, CliError> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let req = make(id);
        let tag = req.tag();
        self.send(&req)?;

        loop {
            let line = self.read_line()?;
            let value: Value = serde_json::from_str(&line).map_err(|err| {
                CliError::protocol(format!("server sent an unparseable line: {err}"))
                    .with_hint(truncate(&line, 200))
            })?;
            match value.get("t").and_then(Value::as_str) {
                Some("ok") | Some("err") => {}
                Some(other) if other.starts_with("ev.") => {
                    // Events are legal at any time once subscribed; `st` never
                    // subscribes, but a server is free to push shutdown notices.
                    if let Ok(ev) = serde_json::from_value::<Ev>(value) {
                        tracing::debug!(?ev, "control event");
                    }
                    continue;
                }
                _ => {
                    tracing::debug!(line = %truncate(&line, 200), "ignoring unknown control line");
                    continue;
                }
            }

            let res: AnyRes = serde_json::from_value(value).map_err(|err| {
                CliError::protocol(format!("server sent a malformed response: {err}"))
                    .with_hint(truncate(&line, 200))
            })?;
            if res.id() != id {
                tracing::debug!(got = res.id(), want = id, "response for another request");
                continue;
            }
            return match res {
                Res::Ok { result, .. } => Ok(result),
                Res::Err { error, .. } => Err(CliError::new(
                    match error.code {
                        ErrorCode::NotFound => ExitCode::NotFound,
                        ErrorCode::BadRequest | ErrorCode::Unsupported => ExitCode::Protocol,
                        _ => ExitCode::Refused,
                    },
                    format!("{tag} failed: {}", error.message),
                )),
            };
        }
    }

    /// [`ControlClient::request_raw`] plus deserialisation into `R`.
    pub fn request<R: serde::de::DeserializeOwned>(
        &mut self,
        make: impl FnOnce(ReqId) -> Req,
    ) -> Result<R, CliError> {
        let raw = self.request_raw(make)?;
        serde_json::from_value(raw.clone()).map_err(|err| {
            CliError::protocol(format!("cannot decode the server's result: {err}"))
                .with_hint(truncate(&raw.to_string(), 200))
        })
    }

    fn send(&mut self, msg: &impl serde::Serialize) -> Result<(), CliError> {
        let mut line = serde_json::to_string(msg)
            .map_err(|e| CliError::failure(format!("cannot encode a request: {e}")))?;
        debug_assert!(line.starts_with('{'), "control messages are JSON objects");
        if line.len() > MAX_CONTROL_LINE {
            return Err(CliError::protocol(format!(
                "request is {} bytes, over the {MAX_CONTROL_LINE} byte line limit",
                line.len()
            )));
        }
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .and_then(|()| self.writer.flush())
            .map_err(|e| CliError::no_server(format!("control connection lost while writing: {e}")))
    }

    fn read_line(&mut self) -> Result<String, CliError> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Err(CliError::no_server(
                "the server closed the control connection",
            )),
            Ok(_) => Ok(line.trim_end_matches(['\n', '\r']).to_owned()),
            Err(err) => Err(CliError::no_server(format!(
                "control connection lost while reading: {err}"
            ))),
        }
    }
}

fn version_mismatch(server: st_proto::ProtoVersion) -> CliError {
    CliError::protocol(format!(
        "protocol major mismatch: server speaks {server}, this st speaks {PROTO_VERSION}"
    ))
    .with_hint("restart the server so both sides run the same build")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_is_a_tool_hello_on_one_line() {
        let line = serde_json::to_string(&Handshake::Hello(Hello {
            proto_version: PROTO_VERSION,
            client_kind: ClientKind::Tool,
            build_id: "test".into(),
        }))
        .unwrap();
        assert_eq!(
            line,
            r#"{"t":"hello","proto_version":"1.1","client_kind":"tool","build_id":"test"}"#
        );
        assert!(!line.contains('\n'));
    }

    #[test]
    fn truncation_is_char_safe() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("日本語です", 2), "日本…");
    }

    #[test]
    fn a_major_mismatch_is_a_protocol_error() {
        let err = version_mismatch(st_proto::ProtoVersion::new(2, 0));
        assert_eq!(err.exit, ExitCode::Protocol);
        assert!(err.message.contains("2.0"));
    }
}
