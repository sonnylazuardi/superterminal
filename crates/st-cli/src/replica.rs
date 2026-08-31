//! The client-side Surface replica — `docs/plan/02-protocol.md` §6, §7.
//!
//! A [`Replica`] is built from a `Snapshot` (which "replaces the replica
//! wholesale", §7) and advanced by `Delta`s. Only what a text renderer needs
//! is kept: the visible grid, the style table, the cursor, the modes, the
//! title and the history counters. No history *content* is cached — `st probe`
//! never scrolls back.

use st_proto::{
    AbsLine, Cursor, Delta, ExitStatus, Modes, Row, Seq, Snapshot, StyleTable, SurfaceId,
};

/// A gap in the sequence stream (§6.3): the `Delta`'s `since_seq` does not
/// match what we last applied, so state is missing and the client must
/// re-`Attach` with `want_snapshot: true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("sequence gap: delta builds on seq {since_seq} but the replica is at seq {have}")]
pub struct SeqGap {
    /// The `since_seq` the server stamped on the delta.
    pub since_seq: Seq,
    /// The sequence number the replica actually holds.
    pub have: Seq,
}

/// A Surface's visible state, as far as `st probe` cares.
#[derive(Debug, Clone)]
pub struct Replica {
    /// The Surface being mirrored.
    pub surface_id: SurfaceId,
    /// The sequence number of the state held.
    pub seq: Seq,
    /// Grid width in columns.
    pub cols: u16,
    /// Grid height in rows.
    pub rows: u16,
    /// The style table, mirrored verbatim from the server (§5.3).
    pub styles: StyleTable,
    /// Exactly `rows` entries, top to bottom. Rows keep their trimmed tails.
    pub grid: Vec<Row>,
    /// Cursor state.
    pub cursor: Cursor,
    /// Terminal modes.
    pub modes: Modes,
    /// Window title.
    pub title: String,
    /// Id of the oldest retained history line.
    pub history_base: AbsLine,
    /// Number of retained history lines.
    pub history_len: u64,
    /// `Some` once the Surface's process has ended.
    pub exited: Option<ExitStatus>,
}

impl Replica {
    /// Builds a replica from a `Snapshot`, discarding any previous state (§7).
    ///
    /// A style table the server sent malformed (empty, or not starting with
    /// the default style) falls back to a fresh table; §5.3 guarantees index 0
    /// is the default style, so a renderer must not fail on a server bug.
    #[must_use]
    pub fn from_snapshot(snap: &Snapshot) -> Self {
        let mut grid = snap.grid.clone();
        grid.resize(snap.rows as usize, Row::new());
        Self {
            surface_id: snap.surface_id,
            seq: snap.seq,
            cols: snap.cols,
            rows: snap.rows,
            styles: StyleTable::from_wire(&snap.styles).unwrap_or_default(),
            grid,
            cursor: snap.cursor,
            modes: snap.modes,
            title: snap.title.clone(),
            history_base: snap.history_base,
            history_len: snap.history_len,
            exited: snap.exited,
        }
    }

    /// Applies a `Delta` in the order §6.2 mandates: `new_styles`, then
    /// `resized`, then rows, then cursor/modes/title/history.
    ///
    /// Fails with [`SeqGap`] when `since_seq` does not match (§6.3); the
    /// replica is left untouched so the caller can re-attach.
    pub fn apply_delta(&mut self, delta: &Delta) -> Result<(), SeqGap> {
        if delta.since_seq != self.seq {
            return Err(SeqGap {
                since_seq: delta.since_seq,
                have: self.seq,
            });
        }

        for (idx, style) in &delta.new_styles {
            self.styles.set(*idx, *style);
        }
        if let Some((cols, rows)) = delta.resized {
            self.cols = cols;
            self.rows = rows;
            self.grid.clear();
            self.grid.resize(rows as usize, Row::new());
        }
        for dirty in &delta.rows {
            let index = dirty.index as usize;
            if index >= self.grid.len() {
                tracing::warn!(index, rows = self.grid.len(), "dirty row is out of range");
                continue;
            }
            self.grid[index] = dirty.row.clone();
        }
        self.cursor = delta.cursor;
        self.modes = delta.modes;
        if let Some(title) = &delta.title {
            self.title.clone_from(title);
        }
        self.history_base = delta.history_base;
        self.history_len = delta.history_len;
        self.seq = delta.seq;
        Ok(())
    }

    /// The absolute line id of the top visible row: `history_base +
    /// history_len` (§8).
    #[must_use]
    pub fn first_visible_line(&self) -> AbsLine {
        self.history_base.saturating_add(self.history_len)
    }

    /// Total lines the scrollbar would span: history plus the visible grid
    /// (§7, grilling Q25).
    #[must_use]
    pub fn total_lines(&self) -> u64 {
        self.history_len + u64::from(self.rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_proto::{Attrs, Color, DirtyRow, PackedCell, Style, StyleIdx};

    fn row(text: &str) -> Row {
        let mut row = Row::new();
        row.cells = text
            .chars()
            .map(|c| PackedCell::from_char(c, StyleIdx::ZERO))
            .collect();
        row.trim_trailing_blanks();
        row
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            surface_id: SurfaceId(7),
            seq: Seq(5),
            cols: 10,
            rows: 3,
            styles: vec![Style::DEFAULT],
            grid: vec![row("one"), row("two"), Row::new()],
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: "zsh".into(),
            history_base: AbsLine(100),
            history_len: 40,
            view_state: st_proto::ViewState::default(),
            exited: None,
        }
    }

    fn delta(seq: u64, since: u64) -> Delta {
        Delta {
            surface_id: SurfaceId(7),
            seq: Seq(seq),
            since_seq: Seq(since),
            history_base: AbsLine(100),
            history_len: 41,
            resized: None,
            new_styles: Vec::new(),
            rows: Vec::new(),
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: None,
        }
    }

    #[test]
    fn snapshot_replaces_everything_and_pads_the_grid() {
        let mut snap = snapshot();
        snap.rows = 5;
        let replica = Replica::from_snapshot(&snap);
        assert_eq!(replica.grid.len(), 5);
        assert_eq!(replica.seq, Seq(5));
        assert_eq!(replica.first_visible_line(), AbsLine(140));
        assert_eq!(replica.total_lines(), 45);
    }

    #[test]
    fn a_malformed_style_table_falls_back_to_the_default_one() {
        let mut snap = snapshot();
        snap.styles = vec![Style {
            attrs: Attrs::BOLD,
            ..Style::DEFAULT
        }];
        let replica = Replica::from_snapshot(&snap);
        assert_eq!(replica.styles.get(StyleIdx::ZERO), Some(Style::DEFAULT));
    }

    #[test]
    fn deltas_apply_in_order_and_advance_the_seq() {
        let mut replica = Replica::from_snapshot(&snapshot());
        let mut d = delta(6, 5);
        d.new_styles = vec![(
            StyleIdx::new(1),
            Style {
                fg: Color::Indexed(4),
                ..Style::DEFAULT
            },
        )];
        d.rows = vec![DirtyRow {
            index: 2,
            row: row("three"),
        }];
        d.title = Some("vim".into());
        replica.apply_delta(&d).unwrap();

        assert_eq!(replica.seq, Seq(6));
        assert_eq!(replica.title, "vim");
        assert_eq!(replica.history_len, 41);
        assert_eq!(replica.grid[2].cells.len(), 5);
        assert_eq!(
            replica.styles.get(StyleIdx::new(1)).unwrap().fg,
            Color::Indexed(4)
        );
    }

    #[test]
    fn a_resize_clears_the_grid_before_the_rows_land() {
        let mut replica = Replica::from_snapshot(&snapshot());
        let mut d = delta(6, 5);
        d.resized = Some((20, 2));
        d.rows = vec![DirtyRow {
            index: 0,
            row: row("resized"),
        }];
        replica.apply_delta(&d).unwrap();
        assert_eq!((replica.cols, replica.rows), (20, 2));
        assert_eq!(replica.grid.len(), 2);
        assert_eq!(replica.grid[1], Row::new());
    }

    #[test]
    fn a_sequence_gap_is_detected_and_leaves_the_replica_alone() {
        let mut replica = Replica::from_snapshot(&snapshot());
        let mut d = delta(9, 8);
        d.title = Some("should not apply".into());
        let err = replica.apply_delta(&d).unwrap_err();
        assert_eq!(
            err,
            SeqGap {
                since_seq: Seq(8),
                have: Seq(5)
            }
        );
        assert_eq!(replica.title, "zsh");
        assert_eq!(replica.seq, Seq(5));
        assert!(err.to_string().contains("sequence gap"));
    }

    #[test]
    fn an_out_of_range_dirty_row_is_dropped_not_panicked_on() {
        let mut replica = Replica::from_snapshot(&snapshot());
        let mut d = delta(6, 5);
        d.rows = vec![DirtyRow {
            index: 99,
            row: row("nowhere"),
        }];
        replica.apply_delta(&d).unwrap();
        assert_eq!(replica.grid.len(), 3);
        assert_eq!(replica.seq, Seq(6));
    }
}
