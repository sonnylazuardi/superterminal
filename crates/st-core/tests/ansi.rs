//! ANSI fixtures: bytes in, grid state out.
//!
//! No PTY is involved — these feed the engine directly, which is the whole
//! point of the `VtEngine` boundary (`docs/plan/03-server.md` §5).

mod common;

use std::time::Instant;

use common::{fixture, grid_text, row_text, style_at};
use st_core::ClientId;
use st_proto::{AttachMode, Attrs, CellFlags, Color, CursorShape, DataMsg, Modes, Style};

#[test]
fn plain_text_lands_in_the_right_cells() {
    let mut s = fixture(20, 4);
    s.feed(b"hello\r\nworld");
    let snap = s.snapshot();

    assert_eq!(snap.cols, 20);
    assert_eq!(snap.rows, 4);
    assert_eq!(snap.grid.len(), 4);
    assert_eq!(grid_text(&snap), vec!["hello", "world", "", ""]);
    assert_eq!(
        snap.grid[0].cells.len(),
        5,
        "trailing blanks are trimmed (Q41)"
    );
    assert_eq!(snap.cursor.row, 1);
    assert_eq!(snap.cursor.col, 5);
    assert!(snap.cursor.visible);
}

#[test]
fn sgr_attributes_and_colours() {
    let mut s = fixture(40, 2);
    s.feed(b"\x1b[1;3;4;9;7;2m\x1b[31;44mA\x1b[0mB");
    s.feed(b"\x1b[38;5;200;48;5;17mC");
    s.feed(b"\x1b[38;2;10;20;30mD");
    s.feed(b"\x1b[0m\x1b[4:3m\x1b[58;5;9mE");
    let snap = s.snapshot();

    let a = style_at(&snap, 0, 0);
    assert_eq!(a.fg, Color::Indexed(1), "SGR 31");
    assert_eq!(a.bg, Color::Indexed(4), "SGR 44");
    assert!(a.attrs.contains(Attrs::BOLD));
    assert!(a.attrs.contains(Attrs::ITALIC));
    assert!(a.attrs.contains(Attrs::UNDERLINE));
    assert!(a.attrs.contains(Attrs::STRIKETHROUGH));
    assert!(a.attrs.contains(Attrs::INVERSE));
    assert!(a.attrs.contains(Attrs::DIM));
    assert_eq!(
        a.attrs.underline_kind(),
        0,
        "plain SGR 4 is a single underline"
    );

    assert_eq!(style_at(&snap, 0, 1), Style::DEFAULT, "SGR 0 resets");

    let c = style_at(&snap, 0, 2);
    assert_eq!(c.fg, Color::Indexed(200));
    assert_eq!(c.bg, Color::Indexed(17));

    let d = style_at(&snap, 0, 3);
    assert_eq!(d.fg, Color::Rgb(10, 20, 30), "SGR 38;2 is truecolor");

    let e = style_at(&snap, 0, 4);
    assert!(e.attrs.contains(Attrs::UNDERLINE));
    assert_eq!(e.attrs.underline_kind(), 2, "4:3 is a curly underline");
    assert_eq!(e.underline_color, Color::Indexed(9), "SGR 58");

    assert_eq!(grid_text(&snap)[0], "ABCDE");
}

#[test]
fn cursor_movement_sequences() {
    let mut s = fixture(20, 6);
    // CUP to row 3, col 5 (1-based).
    s.feed(b"\x1b[3;5HX");
    let snap = s.snapshot();
    assert_eq!(grid_text(&snap)[2], "    X");
    assert_eq!((snap.cursor.row, snap.cursor.col), (2, 5));

    // CUU / CUD / CUF / CUB.
    s.feed(b"\x1b[2A\x1b[3C");
    assert_eq!(cursor(&mut s), (0, 8));
    s.feed(b"\x1b[1B\x1b[4D");
    assert_eq!(cursor(&mut s), (1, 4));

    // DECSC / DECRC.
    s.feed(b"\x1b7\x1b[6;1H\x1b8");
    assert_eq!(cursor(&mut s), (1, 4));

    // Cursor shape via DECSCUSR, and DECTCEM hiding it.
    s.feed(b"\x1b[5 q");
    let snap = s.snapshot();
    assert_eq!(snap.cursor.shape, CursorShape::Beam);
    assert!(snap.cursor.blink);
    s.feed(b"\x1b[?25l");
    assert!(!s.snapshot().cursor.visible, "DECTCEM off hides the cursor");
}

fn cursor(s: &mut st_core::Surface) -> (u16, u16) {
    let snap = s.snapshot();
    (snap.cursor.row, snap.cursor.col)
}

#[test]
fn erase_in_line_and_display() {
    let mut s = fixture(10, 3);
    s.feed(b"abcdefghij\r\nklmnopqrst\r\nuvwxyz");

    // EL 0: erase from the cursor to the end of the line.
    s.feed(b"\x1b[1;4H\x1b[0K");
    assert_eq!(grid_text(&s.snapshot())[0], "abc");

    // EL 1: erase from the start of the line to the cursor.
    s.feed(b"\x1b[2;4H\x1b[1K");
    assert_eq!(grid_text(&s.snapshot())[1], "    opqrst");

    // ED 2: erase the whole display.
    s.feed(b"\x1b[2J");
    assert_eq!(grid_text(&s.snapshot()), vec!["", "", ""]);
}

#[test]
fn scroll_region_confines_the_scroll() {
    let mut s = fixture(10, 5);
    s.feed(b"a\r\nb\r\nc\r\nd\r\ne");
    assert_eq!(grid_text(&s.snapshot()), vec!["a", "b", "c", "d", "e"]);

    // DECSTBM rows 2..4, then a linefeed at the bottom of the region.
    s.feed(b"\x1b[2;4r\x1b[4;1H\n");
    assert_eq!(
        grid_text(&s.snapshot()),
        vec!["a", "c", "d", "", "e"],
        "only rows 2..4 scrolled"
    );

    // Resetting the region restores whole-screen scrolling.
    s.feed(b"\x1b[r\x1b[5;1H\n");
    assert_eq!(grid_text(&s.snapshot()), vec!["c", "d", "", "e", ""]);
}

#[test]
fn alt_screen_enter_and_leave() {
    let mut s = fixture(10, 3);
    s.feed(b"primary\r\nlines\r\nhere\r\nscrolled");
    let before = s.snapshot();
    assert!(!before.modes.contains(Modes::ALT_SCREEN));
    assert!(before.history_len > 0, "the primary screen has history");
    let primary_text = grid_text(&before);
    let primary_first_visible = before.first_visible_line();

    s.feed(b"\x1b[?1049h\x1b[H");
    let alt = s.snapshot();
    assert!(alt.modes.contains(Modes::ALT_SCREEN));
    assert_eq!(alt.history_len, 0, "the alternate screen has no history");
    assert_eq!(
        alt.first_visible_line(),
        primary_first_visible,
        "absolute line ids stay anchored across the transition"
    );
    assert_eq!(
        grid_text(&alt),
        vec!["", "", ""],
        "the alt screen starts blank"
    );

    s.feed(b"ALT");
    assert_eq!(grid_text(&s.snapshot())[0], "ALT");

    s.feed(b"\x1b[?1049l");
    let after = s.snapshot();
    assert!(!after.modes.contains(Modes::ALT_SCREEN));
    assert_eq!(
        grid_text(&after),
        primary_text,
        "the primary screen is intact"
    );
    assert_eq!(after.history_len, before.history_len);
    assert_eq!(after.history_base, before.history_base);
}

#[test]
fn osc_0_and_2_set_the_title() {
    let mut s = fixture(10, 2);
    assert_eq!(s.title(), "st", "the default title before any OSC");

    s.feed(b"\x1b]0;icon and window\x07");
    assert_eq!(s.title(), "icon and window");

    s.feed(b"\x1b]2;window only\x1b\\");
    assert_eq!(s.title(), "window only");
    assert_eq!(s.snapshot().title, "window only");

    // An empty payload is an empty title, not a reset: that is what
    // `alacritty_terminal` reports, and what xterm does.
    s.feed(b"\x1b]2;\x07");
    assert_eq!(s.title(), "");

    // A real `ResetTitle` comes from the XTWINOPS title stack: push while no
    // title is set, then pop.
    let mut s = fixture(10, 2);
    s.feed(b"\x1b[22t\x1b]0;pushed\x07");
    assert_eq!(s.title(), "pushed");
    s.feed(b"\x1b[23t");
    assert_eq!(
        s.title(),
        "st",
        "popping an unset title resets to the default"
    );
}

#[test]
fn osc_7_updates_the_working_directory() {
    let mut s = fixture(10, 2);
    let spawn = s.cwd().to_path_buf();

    s.feed(b"\x1b]7;file://box/home/sonny/projects/superterminal\x1b\\");
    assert_eq!(
        s.cwd(),
        std::path::Path::new("/home/sonny/projects/superterminal")
    );
    assert_ne!(s.cwd(), spawn);
    assert_eq!(
        s.take_cwd_change().as_deref(),
        Some(std::path::Path::new("/home/sonny/projects/superterminal"))
    );
    assert_eq!(s.take_cwd_change(), None, "only reported once");
}

#[test]
fn bell_reaches_the_client_as_its_own_message() {
    let mut s = fixture(10, 2);
    let client = ClientId::new(1);
    let now = Instant::now();
    s.attach(client, AttachMode::Active, now);
    s.flush(now); // drain the Attach Snapshot
    s.ack(client, s.seq(), now);

    s.feed(b"ding\x07dong\x07");
    let frames = s.flush(now + std::time::Duration::from_millis(20));
    let bells = frames
        .iter()
        .filter(|f| matches!(f.msg, DataMsg::Bell(_)))
        .count();
    assert_eq!(bells, 1, "bells coalesce into one event");
    assert!(frames.iter().any(|f| matches!(f.msg, DataMsg::Delta(_))));
}

#[test]
fn mode_bits_are_exported() {
    let mut s = fixture(10, 2);
    assert!(s.snapshot().modes.contains(Modes::LINE_WRAP));

    // The three mouse-reporting modes are mutually exclusive upstream, so
    // each is checked on its own.
    s.feed(b"\x1b[?1000h");
    assert!(s.snapshot().modes.contains(Modes::MOUSE_CLICK));
    s.feed(b"\x1b[?1002h");
    let modes = s.snapshot().modes;
    assert!(modes.contains(Modes::MOUSE_DRAG) && !modes.contains(Modes::MOUSE_CLICK));
    s.feed(b"\x1b[?1003h");
    let modes = s.snapshot().modes;
    assert!(modes.contains(Modes::MOUSE_MOTION) && !modes.contains(Modes::MOUSE_DRAG));

    s.feed(b"\x1b[?2004h\x1b[?1006h\x1b[?1h\x1b[?1004h");
    let modes = s.snapshot().modes;
    assert!(modes.contains(Modes::BRACKETED_PASTE));
    assert!(modes.contains(Modes::MOUSE_SGR));
    assert!(modes.contains(Modes::APP_CURSOR_KEYS));
    assert!(modes.contains(Modes::FOCUS_EVENTS));
    assert!(modes.mouse_reporting());

    s.feed(b"\x1b[?2004l\x1b[?1l\x1b[?7l");
    let modes = s.snapshot().modes;
    assert!(!modes.contains(Modes::BRACKETED_PASTE));
    assert!(!modes.contains(Modes::APP_CURSOR_KEYS));
    assert!(!modes.contains(Modes::LINE_WRAP), "DECAWM off");
}

#[test]
fn device_attributes_are_answered_on_the_pty() {
    let mut s = fixture(10, 2);
    assert!(s.take_pty_replies().is_empty());

    s.feed(b"\x1b[c");
    let reply = s.take_pty_replies();
    assert!(!reply.is_empty(), "the program must get a DA answer");
    assert!(reply.starts_with(b"\x1b[?"), "got {reply:?}");
    assert!(s.take_pty_replies().is_empty(), "replies are drained once");
}

#[test]
fn wide_characters_and_combining_marks() {
    let mut s = fixture(10, 2);
    s.feed("世界e\u{301}x".as_bytes());
    let snap = s.snapshot();
    let row = &snap.grid[0];

    assert!(row.cells[0].flags.contains(CellFlags::WIDE));
    assert_eq!(row.cells[0].codepoint, '世' as u32);
    assert!(row.cells[1].flags.contains(CellFlags::WIDE_SPACER));
    assert_eq!(row.cells[1].codepoint, 0, "a spacer renders nothing");
    assert!(row.cells[2].flags.contains(CellFlags::WIDE));

    let combining = row.cells[4];
    assert!(combining.flags.contains(CellFlags::GRAPHEME_EXT));
    assert_eq!(row.grapheme(combining), Some("e\u{301}"));
    assert_eq!(row_text(row), "世界e\u{301}x");
}

#[test]
fn soft_wrap_sets_the_wrapped_flag() {
    let mut s = fixture(5, 3);
    s.feed(b"abcdefg\r\nhi");
    let snap = s.snapshot();
    assert_eq!(grid_text(&snap), vec!["abcde", "fg", "hi"]);
    assert!(snap.grid[0].wrapped, "Q41: a soft-wrapped row is marked");
    assert!(!snap.grid[1].wrapped);
}

#[test]
fn reset_clears_everything_and_retires_the_line_ids() {
    let mut s = fixture(10, 3);
    s.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
    let before = s.snapshot();
    assert!(before.history_len > 0);
    let highest = before.first_visible_line().get() + u64::from(before.rows);

    s.reset();
    let after = s.snapshot();
    assert_eq!(grid_text(&after), vec!["", "", ""]);
    assert_eq!(after.history_len, 0);
    assert!(
        after.history_base.get() >= highest,
        "ids are retired, never reused: {} vs {highest}",
        after.history_base.get()
    );
    assert!(after.title == "st");
}
