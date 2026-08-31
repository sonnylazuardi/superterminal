//! `st dump-data` — decode a recorded DATA-plane stream.
//!
//! The file is exactly what went over the socket: an optional leading
//! `0xFF "STD"` magic (`02-protocol.md` §1.2) followed by
//! `u32 len | u16 msg_type | postcard payload` frames (§1.3). Pairs with
//! `SUPERTERMINAL_RECORD=1` on the server.
//!
//! Each frame prints as one line:
//!
//! ```text
//! #0    0x0100 Snapshot        1234 B  surface=9 seq=77 80x24 styles=2 rows=24 title="zsh"
//! ```

use std::io::{Read, Write};
use std::path::Path;

use serde_json::{json, Value};
use st_proto::DataMsg;

use crate::dataplane::{decode_recording, RecordedFrame};
use crate::exit::{CliError, Result};

/// Runs the command. `-` reads stdin.
pub fn run(path: &Path, json: bool, limit: usize, out: &mut dyn Write) -> Result<()> {
    let bytes = read_input(path)?;
    let (frames, err) = decode_recording(&bytes);

    let shown = if limit == 0 {
        frames.len()
    } else {
        limit.min(frames.len())
    };
    for (index, frame) in frames.iter().take(shown).enumerate() {
        let line = if json {
            frame_json(index, frame).to_string()
        } else {
            frame_line(index, frame)
        };
        writeln!(out, "{line}").map_err(|e| CliError::failure(format!("cannot write: {e}")))?;
    }
    if shown < frames.len() {
        writeln!(out, "… {} more frames", frames.len() - shown)
            .map_err(|e| CliError::failure(format!("cannot write: {e}")))?;
    }

    match err {
        // A recording cut off mid-frame is the normal result of killing the
        // server, so it is a warning rather than a failure — but only if we
        // decoded something first.
        Some(err) if frames.is_empty() => Err(err),
        Some(err) => {
            eprintln!("st: warning: {err}");
            Ok(())
        }
        None => Ok(()),
    }
}

fn read_input(path: &Path) -> Result<Vec<u8>> {
    if path.as_os_str() == "-" {
        let mut bytes = Vec::new();
        std::io::stdin()
            .read_to_end(&mut bytes)
            .map_err(|e| CliError::failure(format!("cannot read stdin: {e}")))?;
        return Ok(bytes);
    }
    std::fs::read(path).map_err(|e| {
        CliError::failure(format!("cannot read {}: {e}", path.display()))
            .with_hint("set SUPERTERMINAL_RECORD=1 on the server to produce one")
    })
}

/// One human-readable line for a recorded frame.
#[must_use]
pub fn frame_line(index: usize, frame: &RecordedFrame) -> String {
    let name = frame
        .msg
        .as_ref()
        .map_or("<undecodable>", |m| message_name(m));
    let detail = frame.msg.as_ref().map_or_else(String::new, describe);
    format!(
        "#{index:<4} 0x{:04X} {name:<14} {:>7} B  {detail}",
        frame.msg_type, frame.payload_len
    )
    .trim_end()
    .to_string()
}

/// The `--json` form of a recorded frame, body included.
#[must_use]
pub fn frame_json(index: usize, frame: &RecordedFrame) -> Value {
    json!({
        "index": index,
        "msg_type": frame.msg_type,
        "name": frame.msg.as_ref().map_or("<undecodable>", |m| message_name(m)),
        "payload_len": frame.payload_len,
        "summary": frame.msg.as_ref().map(describe),
        "message": frame.msg.as_ref().and_then(body_json),
    })
}

/// The same line `dump-data` prints, for `st probe --dump`.
#[must_use]
pub fn summarize(index: usize, msg: &DataMsg) -> String {
    format!(
        "#{index:<4} 0x{:04X} {:<14}          {}",
        msg.msg_type(),
        message_name(msg),
        describe(msg)
    )
    .trim_end()
    .to_string()
}

/// The struct name behind a `msg_type`.
#[must_use]
pub fn message_name(msg: &DataMsg) -> &'static str {
    match msg {
        DataMsg::Hello(_) => "Hello",
        DataMsg::HelloAck(_) => "HelloAck",
        DataMsg::Reject(_) => "Reject",
        DataMsg::Attach(_) => "Attach",
        DataMsg::Detach(_) => "Detach",
        DataMsg::Input(_) => "Input",
        DataMsg::Resize(_) => "Resize",
        DataMsg::FetchHistory(_) => "FetchHistory",
        DataMsg::Ack(_) => "Ack",
        DataMsg::SetViewState(_) => "SetViewState",
        DataMsg::Snapshot(_) => "Snapshot",
        DataMsg::Delta(_) => "Delta",
        DataMsg::History(_) => "History",
        DataMsg::SurfaceExited(_) => "SurfaceExited",
        DataMsg::Bell(_) => "Bell",
        DataMsg::Detached(_) => "Detached",
        DataMsg::DataError(_) => "DataError",
    }
}

/// A one-line summary of a message's contents.
#[must_use]
pub fn describe(msg: &DataMsg) -> String {
    match msg {
        DataMsg::Hello(m) => format!(
            "proto={} kind={:?} build={:?}",
            m.proto_version, m.client_kind, m.build_id
        ),
        DataMsg::HelloAck(m) => format!(
            "proto={} build={:?} pid={} revision={}",
            m.proto_version, m.server_build_id, m.server_pid, m.workspace_revision
        ),
        DataMsg::Reject(m) => format!("reason={:?} message={:?}", m.reason, m.message),
        DataMsg::Attach(m) => format!(
            "surface={} mode={:?} want_snapshot={} known_seq={}",
            m.surface_id, m.mode, m.want_snapshot, m.known_seq
        ),
        DataMsg::Detach(m) => format!("surface={}", m.surface_id),
        DataMsg::Input(m) => format!("surface={} bytes={}", m.surface_id, m.bytes.len()),
        DataMsg::Resize(m) => format!("surface={} {}x{}", m.surface_id, m.cols, m.rows),
        DataMsg::FetchHistory(m) => format!(
            "surface={} from_line={} count={}",
            m.surface_id, m.from_line, m.count
        ),
        DataMsg::Ack(m) => format!("surface={} seq={}", m.surface_id, m.seq),
        DataMsg::SetViewState(m) => format!(
            "surface={} scroll_offset={} selection={}",
            m.surface,
            m.scroll_offset
                .map_or_else(|| "bottom".to_string(), |line| line.to_string()),
            m.selection.map_or_else(
                || "none".to_string(),
                |s| format!(
                    "{:?} {}:{}..{}:{}",
                    s.kind, s.anchor.line, s.anchor.col, s.head.line, s.head.col
                )
            ),
        ),
        DataMsg::Snapshot(m) => format!(
            "surface={} seq={} {}x{} styles={} rows={} history={}+{} title={:?}{}",
            m.surface_id,
            m.seq,
            m.cols,
            m.rows,
            m.styles.len(),
            m.grid.len(),
            m.history_base,
            m.history_len,
            m.title,
            m.exited.map_or(String::new(), |e| format!(
                " exited(code={:?},signal={:?})",
                e.code, e.signal
            )),
        ),
        DataMsg::Delta(m) => format!(
            "surface={} seq={} since={} rows={} new_styles={} history={}+{}{}{}",
            m.surface_id,
            m.seq,
            m.since_seq,
            m.rows.len(),
            m.new_styles.len(),
            m.history_base,
            m.history_len,
            m.resized
                .map_or(String::new(), |(c, r)| format!(" resized={c}x{r}")),
            m.title
                .as_ref()
                .map_or(String::new(), |t| format!(" title={t:?}")),
        ),
        DataMsg::History(m) => format!(
            "surface={} from_line={} base={} rows={}",
            m.surface_id,
            m.from_line,
            m.history_base,
            m.rows.len()
        ),
        DataMsg::SurfaceExited(m) => format!(
            "surface={} seq={} code={:?} signal={:?}",
            m.surface_id, m.seq, m.status.code, m.status.signal
        ),
        DataMsg::Bell(m) => format!("surface={}", m.surface_id),
        DataMsg::Detached(m) => format!("surface={} reason={:?}", m.surface_id, m.reason),
        DataMsg::DataError(m) => format!(
            "surface={:?} code=0x{:04X} message={:?}",
            m.surface_id.map(|s| s.get()),
            m.code,
            m.message
        ),
    }
}

/// The full decoded body as JSON, for `--json`.
fn body_json(msg: &DataMsg) -> Option<Value> {
    let value = match msg {
        DataMsg::Hello(m) => serde_json::to_value(m),
        DataMsg::HelloAck(m) => serde_json::to_value(m),
        DataMsg::Reject(m) => serde_json::to_value(m),
        DataMsg::Attach(m) => serde_json::to_value(m),
        DataMsg::Detach(m) => serde_json::to_value(m),
        DataMsg::Input(m) => serde_json::to_value(m),
        DataMsg::Resize(m) => serde_json::to_value(m),
        DataMsg::FetchHistory(m) => serde_json::to_value(m),
        DataMsg::Ack(m) => serde_json::to_value(m),
        DataMsg::SetViewState(m) => serde_json::to_value(m),
        DataMsg::Snapshot(m) => serde_json::to_value(m),
        DataMsg::Delta(m) => serde_json::to_value(m),
        DataMsg::History(m) => serde_json::to_value(m),
        DataMsg::SurfaceExited(m) => serde_json::to_value(m),
        DataMsg::Bell(m) => serde_json::to_value(m),
        DataMsg::Detached(m) => serde_json::to_value(m),
        DataMsg::DataError(m) => serde_json::to_value(m),
    };
    value.ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_proto::{
        AbsLine, Ack, Attach, AttachMode, Bell, Cursor, DataError, Delta, Detach, DetachReason,
        Detached, DirtyRow, ExitStatus, FetchHistory, History, Input, Modes, PackedCell, Resize,
        Row, Seq, Snapshot, Style, StyleIdx, SurfaceExited, SurfaceId, ViewState,
        DATA_ERR_SURFACE_EXITED,
    };
    // `SetViewState` is not re-exported at the crate root (yet); reach into
    // the module it lives in.
    use st_proto::data::SetViewState;
    use st_proto::{AbsPoint, Selection, SelectionKind};

    fn frame(msg: DataMsg) -> RecordedFrame {
        let payload_len = msg.to_payload().unwrap().len();
        RecordedFrame {
            msg_type: msg.msg_type(),
            payload_len,
            msg: Some(msg),
        }
    }

    fn snapshot() -> DataMsg {
        let mut row = Row::new();
        row.cells = "hi"
            .chars()
            .map(|c| PackedCell::from_char(c, StyleIdx::ZERO))
            .collect();
        DataMsg::Snapshot(Box::new(Snapshot {
            surface_id: SurfaceId(9),
            seq: Seq(77),
            cols: 80,
            rows: 2,
            styles: vec![Style::DEFAULT, Style::DEFAULT],
            grid: vec![row, Row::new()],
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: "zsh".into(),
            history_base: AbsLine(100),
            history_len: 40,
            view_state: ViewState::default(),
            exited: None,
        }))
    }

    #[test]
    fn a_snapshot_line_names_every_dimension() {
        let f = frame(snapshot());
        assert_eq!(
            frame_line(0, &f),
            format!(
                "#0    0x0100 Snapshot       {:>7} B  surface=9 seq=77 80x2 styles=2 rows=2 \
                 history=100+40 title=\"zsh\"",
                f.payload_len
            )
        );
    }

    #[test]
    fn a_delta_line_shows_the_gap_detector_fields() {
        let msg = DataMsg::Delta(Box::new(Delta {
            surface_id: SurfaceId(9),
            seq: Seq(78),
            since_seq: Seq(77),
            history_base: AbsLine(100),
            history_len: 41,
            resized: Some((100, 30)),
            new_styles: vec![(StyleIdx::new(1), Style::DEFAULT)],
            rows: vec![DirtyRow {
                index: 0,
                row: Row::new(),
            }],
            cursor: Cursor::default(),
            modes: Modes::empty(),
            title: Some("vim".into()),
        }));
        assert!(describe(&msg).starts_with(
            "surface=9 seq=78 since=77 rows=1 new_styles=1 history=100+41 resized=100x30 \
             title=\"vim\""
        ));
    }

    #[test]
    fn every_message_kind_has_a_name_and_a_summary() {
        let msgs = vec![
            DataMsg::Hello(st_proto::Hello {
                proto_version: st_proto::PROTO_VERSION,
                client_kind: st_proto::ClientKind::Data,
                build_id: "b".into(),
            }),
            DataMsg::HelloAck(st_proto::HelloAck {
                proto_version: st_proto::PROTO_VERSION,
                server_build_id: "s".into(),
                workspace_revision: 1,
                server_pid: 2,
            }),
            DataMsg::Reject(st_proto::Reject {
                reason: st_proto::RejectReason::NotHello,
                message: "m".into(),
                server_version: st_proto::PROTO_VERSION,
            }),
            DataMsg::Attach(Attach {
                surface_id: SurfaceId(1),
                mode: AttachMode::Active,
                want_snapshot: true,
                known_seq: Seq(0),
            }),
            DataMsg::Detach(Detach {
                surface_id: SurfaceId(1),
            }),
            DataMsg::Input(Input {
                surface_id: SurfaceId(1),
                bytes: b"ls\r".to_vec(),
            }),
            DataMsg::Resize(Resize {
                surface_id: SurfaceId(1),
                cols: 80,
                rows: 24,
            }),
            DataMsg::FetchHistory(FetchHistory {
                surface_id: SurfaceId(1),
                from_line: AbsLine(5),
                count: 100,
            }),
            DataMsg::Ack(Ack {
                surface_id: SurfaceId(1),
                seq: Seq(3),
            }),
            DataMsg::SetViewState(SetViewState {
                surface: SurfaceId(1),
                scroll_offset: Some(AbsLine(1024)),
                selection: Some(Selection {
                    kind: SelectionKind::Block,
                    anchor: AbsPoint {
                        line: AbsLine(10),
                        col: 0,
                    },
                    head: AbsPoint {
                        line: AbsLine(11),
                        col: 7,
                    },
                }),
            }),
            snapshot(),
            DataMsg::History(Box::new(History {
                surface_id: SurfaceId(1),
                from_line: AbsLine(5),
                history_base: AbsLine(0),
                rows: vec![Row::new()],
            })),
            DataMsg::SurfaceExited(SurfaceExited {
                surface_id: SurfaceId(1),
                seq: Seq(9),
                status: ExitStatus {
                    code: Some(0),
                    signal: None,
                },
            }),
            DataMsg::Bell(Bell {
                surface_id: SurfaceId(1),
            }),
            DataMsg::Detached(Detached {
                surface_id: SurfaceId(1),
                reason: DetachReason::ServerShutdown,
            }),
            DataMsg::DataError(DataError {
                surface_id: Some(SurfaceId(1)),
                code: DATA_ERR_SURFACE_EXITED,
                message: "surface_exited".into(),
            }),
        ];
        for msg in &msgs {
            assert!(!message_name(msg).is_empty());
            assert!(!describe(msg).is_empty(), "no summary for {msg:?}");
            assert!(body_json(msg).is_some(), "no json body for {msg:?}");
        }
        // Every kind but Delta, which is covered above.
        assert_eq!(msgs.len(), 16);

        // §3.2's View State rides the data plane too (msg_type 0x0016).
        assert_eq!(
            describe(&msgs[9]),
            "surface=1 scroll_offset=1024 selection=Block 10:0..11:7"
        );
        assert_eq!(
            describe(&DataMsg::SetViewState(SetViewState {
                surface: SurfaceId(1),
                scroll_offset: None,
                selection: None,
            })),
            "surface=1 scroll_offset=bottom selection=none"
        );
    }

    #[test]
    fn an_undecodable_frame_still_prints() {
        let f = RecordedFrame {
            msg_type: 0x0104,
            payload_len: 0,
            msg: None,
        };
        assert_eq!(frame_line(3, &f), "#3    0x0104 <undecodable>        0 B");
        let value = frame_json(3, &f);
        assert_eq!(value["name"], "<undecodable>");
        assert_eq!(value["message"], Value::Null);
    }

    #[test]
    fn json_carries_the_decoded_body() {
        let value = frame_json(0, &frame(snapshot()));
        assert_eq!(value["msg_type"], 0x0100);
        assert_eq!(value["name"], "Snapshot");
        assert_eq!(value["message"]["title"], "zsh");
        assert_eq!(
            value["message"]["grid"][0]["cells"][0]["codepoint"],
            'h' as u32
        );
    }

    #[test]
    fn probe_dump_lines_use_the_same_vocabulary() {
        let line = summarize(2, &snapshot());
        assert!(line.starts_with("#2    0x0100 Snapshot"));
        assert!(line.contains("seq=77"));
    }
}
