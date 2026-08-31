//! Property tests for the invariants that the rest of the system leans on.

mod common;

use common::{fixture_with_scrollback, row_text};
use proptest::prelude::*;
use st_core::{DirtySet, SurfaceUpdate};
use st_proto::AbsLine;

/// Bytes that exercise the parser without being pure noise: printable text,
/// newlines, and a handful of real escape sequences.
fn vt_bytes() -> impl Strategy<Value = Vec<u8>> {
    let atom = prop_oneof![
        20 => any::<u8>().prop_map(|b| vec![b'a' + (b % 26)]),
        6 => Just(b"\r\n".to_vec()),
        2 => Just(b"\x1b[2J".to_vec()),
        2 => Just(b"\x1b[H".to_vec()),
        2 => Just(b"\x1b[1;32m".to_vec()),
        1 => Just(b"\x1b[?1049h".to_vec()),
        1 => Just(b"\x1b[?1049l".to_vec()),
        1 => Just(b"\x1b[3;5r".to_vec()),
        1 => Just(b"\x07".to_vec()),
        1 => Just("世".as_bytes().to_vec()),
    ];
    proptest::collection::vec(atom, 0..80).prop_map(|chunks| chunks.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// The absolute coordinate space only ever moves forward, and the ring
    /// never keeps more than it was configured to (grilling Q39/Q40).
    #[test]
    fn absolute_line_space_only_moves_forward(chunks in proptest::collection::vec(vt_bytes(), 1..8)) {
        const SCROLLBACK: usize = 32;
        let mut s = fixture_with_scrollback(24, 5, SCROLLBACK);
        let mut last_first_visible = s.history_base().get() + s.history_len();

        for chunk in chunks {
            s.feed(&chunk);
            let base = s.history_base().get();
            let len = s.history_len();
            prop_assert!(len <= SCROLLBACK as u64, "ring kept {len} > {SCROLLBACK}");
            // Feeding output only ever pushes lines *out* of the viewport, so
            // the id of the first visible line never goes backwards — not even
            // across an alternate-screen transition, which is anchored to it.
            prop_assert!(
                base + len >= last_first_visible,
                "first visible line went backwards: {} < {last_first_visible}",
                base + len
            );
            last_first_visible = base + len;
        }
    }

    /// Reflow is off (grilling Q40), so no resize may ever change what an
    /// absolute line id means, and the trim point never moves backwards.
    #[test]
    fn resizing_never_renumbers(
        lines in 1usize..40,
        sizes in proptest::collection::vec((8u16..60, 2u16..20), 1..6),
    ) {
        let mut s = fixture_with_scrollback(40, 8, 64);
        for i in 0..lines {
            s.feed(format!("l{i:03}\r\n").as_bytes());
        }

        // Remember what every addressable id holds right now.
        let base0 = s.history_base().get();
        let page = s.history(AbsLine::new(base0), 400);
        let before: Vec<(u64, String)> = page
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| (base0 + i as u64, row_text(row)))
            .collect();

        let mut last_base = base0;
        for (cols, rows) in sizes {
            s.resize(cols, rows).unwrap();
            prop_assert_eq!(s.size(), (cols, rows));

            let base = s.history_base().get();
            prop_assert!(base >= last_base, "the trim point moved backwards");
            last_base = base;

            for (id, text) in &before {
                if *id < base {
                    continue; // evicted, and eviction is allowed
                }
                let page = s.history(AbsLine::new(*id), 1);
                let Some(row) = page.rows.first() else {
                    continue; // beyond the (possibly shrunk) grid
                };
                prop_assert_eq!(
                    &row_text(row),
                    text,
                    "absolute id {} changed meaning after a resize",
                    id
                );
            }

            let snapshot = s.snapshot();
            prop_assert_eq!(snapshot.grid.len(), rows as usize);
            prop_assert_eq!(snapshot.rows, rows);
            prop_assert_eq!(snapshot.cols, cols);
            prop_assert!(snapshot.cursor.row < rows);
            prop_assert!(snapshot.cursor.col < cols);
        }
    }

    /// Every history line the engine hands out is addressable by its id, and
    /// the page it returns starts where it says it does.
    #[test]
    fn history_pages_are_addressable(count in 1u32..40) {
        let mut s = fixture_with_scrollback(20, 3, 40);
        for i in 0..30u32 {
            s.feed(format!("l{i:03}\r\n").as_bytes());
        }
        let base = s.history_base();
        let page = s.history(base, count);
        prop_assert_eq!(page.from_line, base);
        prop_assert!(page.rows.len() <= count as usize);
        for (offset, row) in page.rows.iter().enumerate() {
            let id = AbsLine::new(base.get() + offset as u64);
            let single = s.history(id, 1);
            prop_assert_eq!(single.rows.len(), 1);
            prop_assert_eq!(row_text(&single.rows[0]), row_text(row));
        }
    }

    /// Interning is a pure function of the styles seen so far, so two Surfaces
    /// fed identical bytes produce identical tables.
    #[test]
    fn interning_is_deterministic_across_surfaces(chunks in proptest::collection::vec(vt_bytes(), 1..5)) {
        let mut a = fixture_with_scrollback(30, 6, 16);
        let mut b = fixture_with_scrollback(30, 6, 16);
        for chunk in &chunks {
            a.feed(chunk);
            b.feed(chunk);
            let _ = a.take_update();
            let _ = b.take_update();
        }
        prop_assert_eq!(a.styles().as_slice(), b.styles().as_slice());
        prop_assert_eq!(a.snapshot(), b.snapshot());
    }

    /// A Delta only ever names rows that exist.
    #[test]
    fn deltas_stay_inside_the_grid(chunks in proptest::collection::vec(vt_bytes(), 1..6)) {
        let mut s = fixture_with_scrollback(18, 4, 16);
        for chunk in chunks {
            s.feed(&chunk);
            match s.take_update() {
                SurfaceUpdate::Delta(delta) => {
                    for row in &delta.rows {
                        prop_assert!(row.index < 4, "row {} outside a 4-row grid", row.index);
                        prop_assert!(row.row.cells.len() <= 18);
                    }
                }
                SurfaceUpdate::Snapshot(snapshot) => {
                    prop_assert_eq!(snapshot.grid.len(), 4);
                }
                SurfaceUpdate::Idle => {}
            }
        }
    }

    /// `DirtySet` is a set: union is idempotent, commutative, and iteration
    /// agrees with `contains`.
    #[test]
    fn dirty_set_behaves_like_a_set(
        rows in 1usize..200,
        a in proptest::collection::vec(0usize..200, 0..40),
        b in proptest::collection::vec(0usize..200, 0..40),
    ) {
        let build = |src: &[usize]| {
            let mut set = DirtySet::new(rows);
            for &i in src {
                set.set(i);
            }
            set
        };
        let mut ab = build(&a);
        ab.union_with(&build(&b));
        let mut ba = build(&b);
        ba.union_with(&build(&a));
        prop_assert_eq!(ab.iter().collect::<Vec<_>>(), ba.iter().collect::<Vec<_>>());

        let mut again = ab.clone();
        again.union_with(&ab);
        prop_assert_eq!(again, ab.clone());

        let expected: std::collections::BTreeSet<usize> = a
            .iter()
            .chain(b.iter())
            .copied()
            .filter(|i| *i < rows)
            .collect();
        prop_assert_eq!(ab.iter().collect::<Vec<_>>(), expected.iter().copied().collect::<Vec<_>>());
        prop_assert_eq!(ab.count(), expected.len());
        for i in 0..rows {
            prop_assert_eq!(ab.contains(i), expected.contains(&i));
        }
    }
}
