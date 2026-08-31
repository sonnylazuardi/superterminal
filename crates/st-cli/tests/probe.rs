//! `st probe` against a fake server that speaks the real DATA plane:
//! `0xFF "STD"` magic, postcard frames, `Attach` → `Snapshot` → `Delta`.

mod common;

use common::{code, stderr, stdout, FakeServer};
use st_proto::{
    AbsLine, Attrs, CellFlags, Color, Cursor, DataMsg, Delta, DirtyRow, ExitStatus, Modes,
    PackedCell, Row, Seq, Snapshot, Style, StyleIdx, SurfaceExited, SurfaceId, ViewState,
};

const SURFACE: SurfaceId = SurfaceId(9);

/// A row of plain ASCII in style 0, with its trailing blanks trimmed (§4.4).
fn text_row(text: &str) -> Row {
    let mut row = Row::new();
    row.cells = text
        .chars()
        .map(|c| PackedCell::from_char(c, StyleIdx::ZERO))
        .collect();
    row.trim_trailing_blanks();
    row
}

/// `世界` as the server encodes it: a WIDE cell plus a WIDE_SPACER (§5.1).
fn wide_row() -> Row {
    let mut row = Row::new();
    for ch in "世界".chars() {
        row.cells
            .push(PackedCell::new(ch as u32, StyleIdx::ZERO, CellFlags::WIDE));
        row.cells
            .push(PackedCell::new(0, StyleIdx::ZERO, CellFlags::WIDE_SPACER));
    }
    row.cells.push(PackedCell::from_char('!', StyleIdx::ZERO));
    row
}

/// A row with a bold-green run in the middle, referencing style 1.
fn styled_row() -> Row {
    let mut row = Row::new();
    for (ch, idx) in [
        ('o', 0u16),
        ('k', 0),
        (':', 0),
        (' ', 0),
        ('P', 1),
        ('A', 1),
        ('S', 1),
        ('S', 1),
    ] {
        row.cells
            .push(PackedCell::from_char(ch, StyleIdx::new(idx)));
    }
    row
}

fn styles() -> Vec<Style> {
    vec![
        Style::DEFAULT,
        Style {
            fg: Color::Indexed(2),
            attrs: Attrs::BOLD,
            ..Style::DEFAULT
        },
    ]
}

fn snapshot() -> Snapshot {
    let mut trailing = text_row("padded");
    // A server that does not trim: `st` must trim these itself.
    trailing.cells.resize(12, PackedCell::BLANK);

    Snapshot {
        surface_id: SURFACE,
        seq: Seq(77),
        cols: 12,
        rows: 6,
        styles: styles(),
        grid: vec![
            text_row("hello world"),
            Row::new(),
            wide_row(),
            styled_row(),
            trailing,
            text_row("last"),
        ],
        cursor: Cursor::default(),
        modes: Modes::LINE_WRAP,
        title: "zsh".into(),
        history_base: AbsLine(100),
        history_len: 940,
        view_state: ViewState::default(),
        exited: None,
    }
}

/// A server that answers `Attach` with one `Snapshot` and nothing else.
fn snapshot_server() -> FakeServer {
    FakeServer::builder()
        .data(|msg| match msg {
            DataMsg::Attach(_) => vec![DataMsg::Snapshot(Box::new(snapshot()))],
            _ => Vec::new(),
        })
        .start()
}

const EXPECTED_SCREEN: &str = "\
hello world

世界!
ok: PASS
padded
last
";

#[test]
fn probe_prints_the_snapshot_as_plain_text() {
    let out = snapshot_server().run(&["probe", "9", "--no-header"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), EXPECTED_SCREEN);
}

#[test]
fn probe_prints_a_header_by_default() {
    let out = snapshot_server().run(&["probe", "9"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("surface 9 seq 77 12x6 history 940 title \"zsh\"\n{EXPECTED_SCREEN}")
    );
}

#[test]
fn probe_color_emits_sgr_from_the_style_table() {
    let out = snapshot_server().run(&["probe", "9", "--no-header", "--color"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "hello world\n\n世界!\nok: \u{1b}[0;1;32mPASS\u{1b}[0m\npadded\nlast\n"
    );
}

#[test]
fn probe_speaks_the_magic_hello_and_attach_in_order() {
    let server = snapshot_server();
    assert_eq!(code(&server.run(&["probe", "9", "--no-header"])), 0);

    let seen: Vec<String> = server
        .wait_seen(3)
        .into_iter()
        .map(|s| match s {
            common::Seen::Data(d) => d,
            common::Seen::Control(c) => panic!("unexpected control line {c}"),
        })
        .collect();

    assert!(seen[0].starts_with("Hello("), "{:?}", seen[0]);
    assert!(seen[0].contains("client_kind: Data"), "{:?}", seen[0]);
    assert!(seen[1].starts_with("Attach("), "{:?}", seen[1]);
    assert!(
        seen[1].contains("surface_id: SurfaceId(9)"),
        "{:?}",
        seen[1]
    );
    assert!(seen[1].contains("mode: Active"), "{:?}", seen[1]);
    assert!(seen[1].contains("want_snapshot: true"), "{:?}", seen[1]);
    assert!(seen[1].contains("known_seq: Seq(0)"), "{:?}", seen[1]);
    // §6.5: the client Acks what it applied.
    assert!(seen[2].starts_with("Ack("), "{:?}", seen[2]);
    assert!(seen[2].contains("seq: Seq(77)"), "{:?}", seen[2]);
}

#[test]
fn probe_passive_mode_is_carried_on_the_attach() {
    let server = snapshot_server();
    assert_eq!(
        code(&server.run(&["probe", "9", "--no-header", "--mode", "passive"])),
        0
    );
    let attach = server
        .wait_seen(2)
        .into_iter()
        .filter_map(|s| match s {
            common::Seen::Data(d) if d.starts_with("Attach(") => Some(d),
            _ => None,
        })
        .next()
        .expect("an Attach");
    assert!(attach.contains("mode: Passive"), "{attach}");
}

#[test]
fn probe_follow_applies_deltas_and_repaints() {
    let server = FakeServer::builder()
        .data(|msg| match msg {
            DataMsg::Attach(_) => vec![DataMsg::Snapshot(Box::new(snapshot()))],
            // The client acks the Snapshot; answer with one Delta.
            DataMsg::Ack(ack) if ack.seq == Seq(77) => {
                vec![DataMsg::Delta(Box::new(Delta {
                    surface_id: SURFACE,
                    seq: Seq(78),
                    since_seq: Seq(77),
                    history_base: AbsLine(100),
                    history_len: 941,
                    resized: None,
                    new_styles: Vec::new(),
                    rows: vec![DirtyRow {
                        index: 0,
                        row: text_row("changed"),
                    }],
                    cursor: Cursor::default(),
                    modes: Modes::LINE_WRAP,
                    title: Some("vim".into()),
                }))]
            }
            _ => Vec::new(),
        })
        .start();

    let out = server.run(&["probe", "9", "--follow", "--max-deltas", "1"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));

    let text = stdout(&out);
    let (first, second) = text.split_once("surface 9 seq 78").expect("two paints");
    assert_eq!(
        first,
        format!("surface 9 seq 77 12x6 history 940 title \"zsh\"\n{EXPECTED_SCREEN}")
    );
    assert_eq!(
        second,
        format!(
            " 12x6 history 941 title \"vim\"\n{}",
            EXPECTED_SCREEN.replacen("hello world", "changed", 1)
        )
    );
}

#[test]
fn probe_re_attaches_after_a_sequence_gap() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ATTACHES: AtomicUsize = AtomicUsize::new(0);
    ATTACHES.store(0, Ordering::SeqCst);

    let server = FakeServer::builder()
        .data(|msg| match msg {
            DataMsg::Attach(_) => {
                ATTACHES.fetch_add(1, Ordering::SeqCst);
                vec![DataMsg::Snapshot(Box::new(snapshot()))]
            }
            DataMsg::Ack(ack) if ack.seq == Seq(77) => {
                let first = ATTACHES.load(Ordering::SeqCst) == 1;
                // §6.3: the first delta claims to build on seq 999, which the
                // client never had. It must re-Attach rather than apply it.
                vec![DataMsg::Delta(Box::new(Delta {
                    surface_id: SURFACE,
                    seq: if first { Seq(1000) } else { Seq(78) },
                    since_seq: if first { Seq(999) } else { Seq(77) },
                    history_base: AbsLine(100),
                    history_len: 940,
                    resized: None,
                    new_styles: Vec::new(),
                    rows: vec![DirtyRow {
                        index: 0,
                        row: text_row("after gap"),
                    }],
                    cursor: Cursor::default(),
                    modes: Modes::LINE_WRAP,
                    title: None,
                }))]
            }
            _ => Vec::new(),
        })
        .start();

    let out = server.run(&["probe", "9", "--follow", "--max-deltas", "1", "--no-header"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(
        ATTACHES.load(Ordering::SeqCst) >= 2,
        "should have re-attached"
    );
    // Snapshot, re-Attach Snapshot, then the good delta: never a corrupt screen.
    assert_eq!(
        stdout(&out),
        format!(
            "{EXPECTED_SCREEN}{EXPECTED_SCREEN}{}",
            EXPECTED_SCREEN.replacen("hello world", "after gap", 1)
        )
    );
}

#[test]
fn probe_reports_an_exited_surface() {
    let server = FakeServer::builder()
        .data(|msg| match msg {
            DataMsg::Attach(_) => vec![DataMsg::Snapshot(Box::new(snapshot()))],
            DataMsg::Ack(_) => vec![DataMsg::SurfaceExited(SurfaceExited {
                surface_id: SURFACE,
                seq: Seq(78),
                status: ExitStatus {
                    code: Some(3),
                    signal: None,
                },
            })],
            _ => Vec::new(),
        })
        .start();

    let out = server.run(&["probe", "9", "--follow", "--no-header"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("{EXPECTED_SCREEN}[surface exited: code 3]\n")
    );
}

#[test]
fn probe_reports_a_destroyed_surface_as_not_found() {
    let server = FakeServer::builder()
        .data(|msg| match msg {
            DataMsg::Attach(_) => vec![DataMsg::DataError(st_proto::DataError {
                surface_id: Some(SurfaceId(99)),
                code: st_proto::DATA_ERR_NOT_ATTACHED,
                message: "surface 99 does not exist".into(),
            })],
            _ => Vec::new(),
        })
        .start();

    let out = server.run(&["probe", "99"]);
    assert_eq!(code(&out), 5);
    assert!(
        stderr(&out).contains("surface 99 does not exist"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn probe_dump_prints_message_summaries_instead_of_the_screen() {
    let out = snapshot_server().run(&["probe", "9", "--dump"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("#0    0x0100 Snapshot"), "{text}");
    assert!(text.contains("0x0100 Snapshot"), "{text}");
    assert!(
        text.contains("surface=9 seq=77 12x6 styles=2 rows=6 history=100+940 title=\"zsh\""),
        "{text}"
    );
    assert!(
        !text.contains("hello world"),
        "no screen in dump mode: {text}"
    );
}

#[test]
fn probe_on_a_dead_socket_exits_three() {
    let out = common::run_st(
        Some(std::path::Path::new("/nonexistent/st.sock")),
        &["probe", "1"],
    );
    assert_eq!(code(&out), 3);
    assert!(
        stderr(&out).contains("no server socket"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_rejected_data_handshake_exits_four() {
    let server = FakeServer::builder().rejecting().start();
    let out = server.run(&["probe", "1"]);
    assert_eq!(code(&out), 4);
    assert!(
        stderr(&out).contains("rejected the data connection"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_server_that_hangs_up_before_the_snapshot_exits_three() {
    let server = FakeServer::builder().silent().start();
    let out = server.run(&["probe", "1"]);
    assert_eq!(code(&out), 3, "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("closed the data connection"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_non_numeric_surface_id_is_a_usage_error() {
    let out = common::run_st(None, &["probe", "banana"]);
    assert_eq!(code(&out), 2);
    let err = stderr(&out);
    assert!(err.contains("invalid value 'banana'"), "{err}");
    assert!(err.contains("SURFACE-ID"), "{err}");
}
