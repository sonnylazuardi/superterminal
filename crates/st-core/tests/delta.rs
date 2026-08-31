//! Damage → Delta correctness, style interning, resize and history.

mod common;

use common::{fixture, fixture_with_scrollback, grid_text, row_text};
use st_core::SurfaceUpdate;
use st_proto::{AbsLine, Color, Style};

fn dirty_indices(update: &SurfaceUpdate) -> Vec<u16> {
    match update {
        SurfaceUpdate::Delta(delta) => delta.rows.iter().map(|r| r.index).collect(),
        other => panic!("expected a Delta, got {other:?}"),
    }
}

#[test]
fn a_write_dirties_exactly_the_rows_it_touched() {
    let mut s = fixture(20, 6);
    // Park the cursor first, then settle, so the only damage below is ours.
    s.feed(b"\x1b[3;1H");
    let _ = s.take_update();
    assert!(matches!(s.take_update(), SurfaceUpdate::Idle), "settled");

    s.feed(b"abc");
    let update = s.take_update();
    assert_eq!(dirty_indices(&update), vec![2]);

    // Two distant rows, plus the row the cursor left behind: alacritty always
    // damages the old and new cursor cells so a stale block is repainted.
    let _ = s.take_update();
    s.feed(b"\x1b[1;1Hx\x1b[5;1Hy");
    assert_eq!(dirty_indices(&s.take_update()), vec![0, 2, 4]);

    assert!(matches!(s.take_update(), SurfaceUpdate::Idle));
}

#[test]
fn a_delta_row_carries_the_whole_row_trimmed() {
    let mut s = fixture(20, 3);
    let _ = s.take_update();
    s.feed(b"\x1b[2;1Hhello world");
    let update = s.take_update();
    let SurfaceUpdate::Delta(delta) = update else {
        panic!("expected a Delta");
    };
    let row = &delta.rows.iter().find(|r| r.index == 1).unwrap().row;
    assert_eq!(row_text(row), "hello world");
    assert_eq!(row.cells.len(), 11, "Q41: trailing blanks are trimmed");
    assert!(!row.wrapped);
    assert_eq!(delta.cursor.row, 1);
    assert_eq!(delta.cursor.col, 11);
}

#[test]
fn sequence_numbers_chain_through_since_seq() {
    let mut s = fixture(10, 3);
    assert!(
        matches!(s.take_update(), SurfaceUpdate::Idle),
        "a fresh Surface has nothing to say until it is written to or attached"
    );

    s.feed(b"x");
    let SurfaceUpdate::Delta(first) = s.take_update() else {
        panic!("expected a Delta");
    };
    assert_eq!(first.since_seq.get() + 1, first.seq.get());

    s.feed(b"a");
    let SurfaceUpdate::Delta(second) = s.take_update() else {
        panic!("expected a Delta");
    };
    assert_eq!(second.since_seq, first.seq);
    assert_eq!(second.seq.get(), first.seq.get() + 1);
    assert_eq!(s.seq(), second.seq);
}

#[test]
fn styles_are_interned_once_and_only_new_ones_ride_along() {
    let mut s = fixture(20, 3);
    let _ = s.take_update();

    s.feed(b"\x1b[31mred");
    let SurfaceUpdate::Delta(first) = s.take_update() else {
        panic!("expected a Delta");
    };
    let red = Style {
        fg: Color::Indexed(1),
        ..Style::DEFAULT
    };
    assert!(first.new_styles.iter().any(|(_, st)| *st == red));
    let red_idx = first
        .new_styles
        .iter()
        .find(|(_, st)| *st == red)
        .unwrap()
        .0;

    // The same style again is not resent, and keeps its index.
    s.feed(b"\x1b[2;1H\x1b[31mmore red");
    let SurfaceUpdate::Delta(second) = s.take_update() else {
        panic!("expected a Delta");
    };
    assert!(
        second.new_styles.is_empty(),
        "an already-sent style is never resent: {:?}",
        second.new_styles
    );
    assert_eq!(second.rows[0].row.cells[0].style_idx, red_idx);

    // A different style is appended.
    s.feed(b"\x1b[3;1H\x1b[32mgreen");
    let SurfaceUpdate::Delta(third) = s.take_update() else {
        panic!("expected a Delta");
    };
    assert_eq!(third.new_styles.len(), 1);
    assert_eq!(
        third.new_styles[0].1,
        Style {
            fg: Color::Indexed(2),
            ..Style::DEFAULT
        }
    );
    assert_ne!(third.new_styles[0].0, red_idx);
}

#[test]
fn a_snapshot_carries_the_whole_table_and_the_next_delta_carries_nothing() {
    let mut s = fixture(20, 3);
    s.feed(b"\x1b[31mred\x1b[32mgreen");
    let snapshot = s.snapshot();
    assert!(snapshot.styles.len() >= 3, "default + red + green");
    assert_eq!(snapshot.styles[0], Style::DEFAULT);

    s.feed(b"\x1b[2;1H\x1b[31mred again");
    let SurfaceUpdate::Delta(delta) = s.take_update() else {
        panic!("expected a Delta");
    };
    assert!(delta.new_styles.is_empty());
}

#[test]
fn overflowing_the_style_table_forces_a_snapshot() {
    let mut s = fixture(10, 3);
    let _ = s.take_update();

    let mut snapshots = 0usize;
    let mut deltas = 0usize;
    for i in 0..4_200u32 {
        let (r, g, b) = ((i >> 16) & 0xff, (i >> 8) & 0xff, i & 0xff);
        s.feed(format!("\x1b[1;1H\x1b[38;2;{r};{g};{b}mX").as_bytes());
        match s.take_update() {
            SurfaceUpdate::Delta(_) => deltas += 1,
            SurfaceUpdate::Snapshot(_) => snapshots += 1,
            SurfaceUpdate::Idle => panic!("a write always produces something"),
        }
    }
    assert!(deltas > 4_000);
    assert_eq!(snapshots, 1, "the 4096-entry cap is hit exactly once here");
    assert!(
        s.styles().generation() >= 1,
        "the table was reset at least once"
    );
    assert!(s.styles().len() < st_proto::STYLE_TABLE_CAP);
    assert!(!s.needs_snapshot(), "the forced Snapshot cleared the latch");
}

#[test]
fn resize_keeps_content_and_line_ids_because_reflow_is_off() {
    let mut s = fixture(20, 4);
    s.feed(b"line1\r\nline2\r\nline3\r\nline4\r\nline5\r\nline6");
    let before = s.snapshot();
    assert!(before.history_len > 0);
    assert_eq!(grid_text(&before), vec!["line3", "line4", "line5", "line6"]);

    // Narrower: nothing is reflowed, so the history keeps its line count and
    // every absolute id keeps its meaning (grilling Q40).
    s.resize(10, 4).unwrap();
    let after = s.snapshot();
    assert_eq!(after.cols, 10);
    assert_eq!(after.history_base, before.history_base);
    assert_eq!(after.history_len, before.history_len);
    assert_eq!(after.first_visible_line(), before.first_visible_line());
    assert_eq!(grid_text(&after), grid_text(&before));

    // Wider again: still no reflow, so a soft-wrapped pair stays two rows.
    s.resize(40, 4).unwrap();
    let wider = s.snapshot();
    assert_eq!(wider.cols, 40);
    assert_eq!(wider.history_base, before.history_base);
    assert_eq!(wider.history_len, before.history_len);
    assert_eq!(grid_text(&wider), grid_text(&before));
}

#[test]
fn a_wrapped_line_is_not_rejoined_by_a_widening_resize() {
    let mut s = fixture(10, 3);
    s.feed(b"abcdefghijklmno");
    let before = s.snapshot();
    assert_eq!(grid_text(&before), vec!["abcdefghij", "klmno", ""]);
    assert!(before.grid[0].wrapped);

    s.resize(40, 3).unwrap();
    let after = s.snapshot();
    assert_eq!(
        grid_text(&after),
        vec!["abcdefghij", "klmno", ""],
        "Q40: reflow is off, so the rows stay split"
    );
    assert_eq!(after.history_len, before.history_len);
}

#[test]
fn shrinking_the_row_count_pushes_lines_into_history_without_renumbering() {
    let mut s = fixture(20, 5);
    s.feed(b"a\r\nb\r\nc\r\nd\r\ne");
    let before = s.snapshot();
    assert_eq!(before.history_len, 0);
    let first_visible = before.first_visible_line().get();

    s.resize(20, 3).unwrap();
    let after = s.snapshot();
    assert_eq!(after.rows, 3);
    assert_eq!(grid_text(&after), vec!["c", "d", "e"]);
    assert_eq!(after.history_len, 2, "a and b moved into the history");
    assert_eq!(
        after.history_base, before.history_base,
        "nothing was evicted, so no id moved"
    );
    assert_eq!(
        after.history_base.get() + after.history_len,
        first_visible + 2
    );
}

#[test]
fn a_resize_clears_the_selection_and_rides_the_next_delta() {
    use st_proto::{AbsPoint, Selection, SelectionKind, ViewState};
    let mut s = fixture(20, 4);
    s.set_view_state(ViewState {
        scroll_offset: 3,
        selection: Some(Selection {
            kind: SelectionKind::Normal,
            anchor: AbsPoint {
                line: AbsLine::new(0),
                col: 0,
            },
            head: AbsPoint {
                line: AbsLine::new(1),
                col: 4,
            },
        }),
    });
    let _ = s.take_update();

    s.resize(30, 6).unwrap();
    assert!(
        s.view_state().selection.is_none(),
        "Q40: a resize clears the selection"
    );
    let SurfaceUpdate::Delta(delta) = s.take_update() else {
        panic!("expected a Delta");
    };
    assert_eq!(delta.resized, Some((30, 6)));
    assert_eq!(delta.rows.len(), 6, "every row is dirty after a resize");
}

#[test]
fn absolute_ids_stay_stable_while_the_ring_evicts() {
    let mut s = fixture_with_scrollback(20, 3, 5);
    for i in 0..10u32 {
        s.feed(format!("row{i:02}\r\n").as_bytes());
    }

    let base = s.history_base();
    assert!(base.get() > 0, "the ring has evicted something");
    assert_eq!(s.history_len(), 5, "and is capped at the configured size");

    // Read the whole retained history plus the visible grid.
    let page = s.history(base, 64);
    assert_eq!(page.history_base, base);
    assert_eq!(page.from_line, base);
    let texts: Vec<String> = page.rows.iter().map(row_text).collect();
    assert_eq!(texts.len(), 5 + 3, "history + the visible grid");

    // Remember what a specific absolute id holds.
    // Pick a line far enough from the trim point to survive the next round of
    // eviction.
    let probe = AbsLine::new(base.get() + 4);
    let probe_text = row_text(&s.history(probe, 1).rows[0]);
    assert!(probe_text.starts_with("row"), "got {probe_text:?}");

    // Evict some more; the id must still hold the same line.
    for i in 10..13u32 {
        s.feed(format!("row{i:02}\r\n").as_bytes());
    }
    assert!(s.history_base().get() > base.get(), "more was evicted");
    assert_eq!(s.history_len(), 5);
    assert_eq!(
        row_text(&s.history(probe, 1).rows[0]),
        probe_text,
        "an absolute id never changes meaning"
    );

    // A request below the trim point is clamped up to it.
    let clamped = s.history(AbsLine::new(0), 3);
    assert_eq!(clamped.from_line, s.history_base());
    assert_eq!(clamped.rows.len(), 3);

    // A request past the end returns nothing.
    let past = s.history(AbsLine::new(1_000_000), 3);
    assert!(past.rows.is_empty());
}

#[test]
fn history_ids_are_contiguous_and_ordered() {
    let mut s = fixture_with_scrollback(20, 3, 50);
    for i in 0..20u32 {
        s.feed(format!("line{i:02}\r\n").as_bytes());
    }
    let base = s.history_base();
    let page = s.history(base, 1000);
    let texts: Vec<String> = page.rows.iter().map(row_text).collect();
    let expected: Vec<String> = (0..20).map(|i| format!("line{i:02}")).collect();
    assert_eq!(&texts[..20], &expected[..]);
    assert_eq!(base.get(), 0, "nothing was evicted with a 50-line ring");
    assert_eq!(s.history_len(), 18, "21 lines produced, 3 on screen");
}

#[test]
fn a_coalesced_delta_carries_the_latest_state_not_a_replay() {
    let mut s = fixture(20, 3);
    let _ = s.take_update();

    s.feed(b"\x1b[1;1Hfirst");
    s.feed(b"\x1b[1;1H\x1b[Ksecond");
    s.feed(b"\x1b[1;1H\x1b[Kthird");
    let SurfaceUpdate::Delta(delta) = s.take_update() else {
        panic!("expected a Delta");
    };
    let row = &delta.rows.iter().find(|r| r.index == 0).unwrap().row;
    assert_eq!(row_text(row), "third", "Q27: coalesced final state");
}
