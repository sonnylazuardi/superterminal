//! `st dump-data` against a synthetic recording built with the real encoder.

mod common;

use std::path::Path;

use common::{code, run_st, stderr, stdout};
use serde_json::Value;
use st_proto::{
    AbsLine, Ack, Attach, AttachMode, Bell, ClientKind, Cursor, DataMsg, Delta, DirtyRow, Hello,
    Modes, PackedCell, Row, Seq, Snapshot, Style, StyleIdx, SurfaceId, ViewState, DATA_MAGIC,
    PROTO_VERSION,
};

const SURFACE: SurfaceId = SurfaceId(9);

fn row(text: &str) -> Row {
    let mut row = Row::new();
    row.cells = text
        .chars()
        .map(|c| PackedCell::from_char(c, StyleIdx::ZERO))
        .collect();
    row
}

fn messages() -> Vec<DataMsg> {
    vec![
        DataMsg::Hello(Hello {
            proto_version: PROTO_VERSION,
            client_kind: ClientKind::Data,
            build_id: "st-cli test".into(),
        }),
        DataMsg::Attach(Attach {
            surface_id: SURFACE,
            mode: AttachMode::Active,
            want_snapshot: true,
            known_seq: Seq(0),
        }),
        DataMsg::Snapshot(Box::new(Snapshot {
            surface_id: SURFACE,
            seq: Seq(77),
            cols: 80,
            rows: 2,
            styles: vec![Style::DEFAULT, Style::DEFAULT],
            grid: vec![row("hi"), Row::new()],
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: "zsh".into(),
            history_base: AbsLine(100),
            history_len: 40,
            view_state: ViewState::default(),
            exited: None,
        })),
        DataMsg::Ack(Ack {
            surface_id: SURFACE,
            seq: Seq(77),
        }),
        DataMsg::Delta(Box::new(Delta {
            surface_id: SURFACE,
            seq: Seq(78),
            since_seq: Seq(77),
            history_base: AbsLine(100),
            history_len: 41,
            resized: None,
            new_styles: Vec::new(),
            rows: vec![DirtyRow {
                index: 1,
                row: row("bye"),
            }],
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: Some("vim".into()),
        })),
        DataMsg::Bell(Bell {
            surface_id: SURFACE,
        }),
    ]
}

/// Writes a recording exactly as `SUPERTERMINAL_RECORD=1` would: magic, then
/// frames.
fn recording(dir: &Path, with_magic: bool, truncate_last: bool) -> std::path::PathBuf {
    let mut wire = Vec::new();
    if with_magic {
        wire.extend_from_slice(&DATA_MAGIC);
    }
    for msg in messages() {
        msg.encode_to(&mut wire).unwrap();
    }
    if truncate_last {
        wire.truncate(wire.len() - 2);
    }
    let path = dir.join("record.bin");
    std::fs::write(&path, wire).unwrap();
    path
}

#[test]
fn every_frame_gets_a_line_with_type_size_and_summary() {
    let dir = tempfile::tempdir().unwrap();
    let path = recording(dir.path(), true, false);
    let out = run_st(None, &["dump-data", path.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));

    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 6, "{text}");

    assert!(
        lines[0].starts_with("#0    0x0001 Hello         "),
        "{}",
        lines[0]
    );
    assert!(lines[0].ends_with("proto=1.1 kind=Data build=\"st-cli test\""));

    assert!(
        lines[1].starts_with("#1    0x0010 Attach        "),
        "{}",
        lines[1]
    );
    assert!(lines[1].ends_with("surface=9 mode=Active want_snapshot=true known_seq=0"));

    assert!(
        lines[2].starts_with("#2    0x0100 Snapshot      "),
        "{}",
        lines[2]
    );
    assert!(
        lines[2].ends_with("surface=9 seq=77 80x2 styles=2 rows=2 history=100+40 title=\"zsh\""),
        "{}",
        lines[2]
    );

    assert!(lines[3].ends_with("surface=9 seq=77"), "{}", lines[3]);
    assert!(
        lines[4].ends_with(
            "surface=9 seq=78 since=77 rows=1 new_styles=0 history=100+41 title=\"vim\""
        ),
        "{}",
        lines[4]
    );
    assert!(lines[5].starts_with("#5    0x0106 Bell"), "{}", lines[5]);

    // The byte count is real: it matches the encoded payload length.
    let snapshot_len = messages()[2].to_payload().unwrap().len();
    assert!(
        lines[2].contains(&format!("{snapshot_len:>7} B")),
        "{}",
        lines[2]
    );
}

#[test]
fn a_recording_without_the_leading_magic_still_decodes() {
    let dir = tempfile::tempdir().unwrap();
    let path = recording(dir.path(), false, false);
    let out = run_st(None, &["dump-data", path.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).lines().count(), 6);
}

#[test]
fn limit_truncates_the_listing_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = recording(dir.path(), true, false);
    let out = run_st(None, &["dump-data", path.to_str().unwrap(), "--limit", "2"]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    assert_eq!(text.lines().count(), 3);
    assert!(text.ends_with("… 4 more frames\n"), "{text}");
}

#[test]
fn json_output_carries_the_decoded_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let path = recording(dir.path(), true, false);
    let out = run_st(None, &["dump-data", path.to_str().unwrap(), "--json"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));

    let docs: Vec<Value> = stdout(&out)
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is one JSON object"))
        .collect();
    assert_eq!(docs.len(), 6);
    assert_eq!(docs[2]["name"], "Snapshot");
    assert_eq!(docs[2]["msg_type"], 0x0100);
    assert_eq!(docs[2]["index"], 2);
    assert_eq!(docs[2]["message"]["title"], "zsh");
    assert_eq!(docs[2]["message"]["cols"], 80);
    assert_eq!(docs[4]["message"]["since_seq"], 77);
    assert_eq!(docs[5]["message"]["surface_id"], 9);
}

#[test]
fn a_truncated_recording_prints_what_it_has_and_warns() {
    let dir = tempfile::tempdir().unwrap();
    let path = recording(dir.path(), true, true);
    let out = run_st(None, &["dump-data", path.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "a partial recording is not a failure");
    assert_eq!(stdout(&out).lines().count(), 5, "the last frame is cut off");
    assert!(
        stderr(&out).contains("trailing bytes are not a complete frame"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_missing_file_exits_one_with_a_hint() {
    let out = run_st(None, &["dump-data", "/nonexistent/record.bin"]);
    assert_eq!(code(&out), 1);
    let err = stderr(&out);
    assert!(err.contains("cannot read /nonexistent/record.bin"), "{err}");
    assert!(err.contains("SUPERTERMINAL_RECORD=1"), "{err}");
}

#[test]
fn a_file_of_junk_exits_four() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("junk.bin");
    // A length header claiming a frame far bigger than the cap.
    std::fs::write(&path, [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x01]).unwrap();
    let out = run_st(None, &["dump-data", path.to_str().unwrap()]);
    assert_eq!(code(&out), 4);
    assert!(stderr(&out).contains("framing error"), "{}", stderr(&out));
}
