//! Per-Surface style interning and the grilling-Q45 overflow policy.
//!
//! `st-proto` owns the [`StyleTable`] data structure (index 0 is always
//! [`Style::DEFAULT`], the cap is [`STYLE_TABLE_CAP`] = 4096, `intern` returns
//! `None` when full). What lives here is the *server* policy on top of it:
//!
//! * remember which entries have not been communicated yet, so a `Delta` can
//!   carry `new_styles` and a client can apply the frame in one pass
//!   (`03-server.md` §4);
//! * on overflow, reset the table, bump the generation and latch a
//!   "needs Snapshot" flag — indices from the old generation are meaningless
//!   to a client, so the next frame on every subscription must be a full
//!   `Snapshot` carrying the whole new table (grilling Q45).
//!
//! Because the table is append-only within a generation, "not yet sent" is
//! just a watermark: every entry from `flushed_len` upwards is new.
//!
//! History rows are never *stored* with indices: `FetchHistory` re-encodes
//! from the engine at request time, so an evicted index can never leak.

use st_proto::{Style, StyleIdx, StyleTable, STYLE_TABLE_CAP};

/// A Surface's style table plus the server-side overflow policy.
#[derive(Debug, Clone)]
pub struct SurfaceStyleTable {
    table: StyleTable,
    /// Number of leading entries the peer is known to have.
    flushed_len: usize,
    generation: u32,
    overflowed: bool,
}

impl Default for SurfaceStyleTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceStyleTable {
    /// A fresh table holding only [`Style::DEFAULT`] at index 0.
    #[must_use]
    pub fn new() -> Self {
        Self {
            table: StyleTable::new(),
            flushed_len: 1,
            generation: 0,
            overflowed: false,
        }
    }

    /// Returns the index of `style`, assigning one if it is new.
    ///
    /// When the table is full and `style` is not already in it, the table is
    /// reset (grilling Q45): the generation advances, [`Self::overflowed`]
    /// latches, and `style` is interned into the fresh table. Callers must
    /// then discard whatever they were building and send a `Snapshot`.
    pub fn intern(&mut self, style: Style) -> StyleIdx {
        if let Some(idx) = self.table.intern(style) {
            return idx;
        }
        self.reset();
        self.table
            .intern(style)
            .expect("a freshly reset style table always has room")
    }

    /// The style-table entries the peer has not seen yet, in index order,
    /// ready for `Delta.new_styles`.
    #[must_use]
    pub fn new_styles(&self) -> Vec<(StyleIdx, Style)> {
        self.table.as_slice()[self.flushed_len..]
            .iter()
            .enumerate()
            .map(|(off, style)| (StyleIdx::new((self.flushed_len + off) as u16), *style))
            .collect()
    }

    /// `true` when the peer is missing at least one entry.
    #[must_use]
    pub fn has_new_styles(&self) -> bool {
        self.flushed_len < self.table.len()
    }

    /// Takes the unsent entries and marks them as sent.
    pub fn take_new(&mut self) -> Vec<(StyleIdx, Style)> {
        let new = self.new_styles();
        self.flushed_len = self.table.len();
        new
    }

    /// Rewinds the watermark so the next frame re-sends every entry.
    ///
    /// Used when a half-built Delta is discarded, and after a reset.
    pub fn rollback_flush_window(&mut self) {
        self.flushed_len = 1;
    }

    /// `true` when the table overflowed since the flag was last taken, i.e. a
    /// full `Snapshot` is required.
    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Takes and clears the overflow flag.
    pub fn take_overflow(&mut self) -> bool {
        std::mem::replace(&mut self.overflowed, false)
    }

    /// How many times the table has been reset. Two indices are comparable
    /// only within one generation.
    #[must_use]
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// Drops every entry but [`Style::DEFAULT`], advances the generation and
    /// latches [`Self::overflowed`].
    pub fn reset(&mut self) {
        self.table.reset();
        self.flushed_len = 1;
        self.generation = self.generation.wrapping_add(1);
        self.overflowed = true;
    }

    /// The whole table in index order, as `Snapshot.styles` carries it.
    #[must_use]
    pub fn as_slice(&self) -> &[Style] {
        self.table.as_slice()
    }

    /// Number of entries, always at least 1.
    #[must_use]
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Always `false`; the table always holds the default style.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// `true` when one more distinct style would trigger the reset.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.table.len() >= STYLE_TABLE_CAP
    }

    /// Marks the whole table as already sent, which is what building a
    /// `Snapshot` does (a Snapshot carries every entry).
    pub fn mark_all_flushed(&mut self) {
        self.flushed_len = self.table.len();
    }

    /// Looks an index up; `None` for an index this generation never assigned.
    #[must_use]
    pub fn get(&self, idx: StyleIdx) -> Option<Style> {
        self.table.get(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_proto::{Attrs, Color};

    fn rgb(i: usize) -> Style {
        Style {
            fg: Color::Rgb((i >> 16) as u8, (i >> 8) as u8, i as u8),
            ..Style::DEFAULT
        }
    }

    #[test]
    fn interning_is_deterministic() {
        let mut a = SurfaceStyleTable::new();
        let mut b = SurfaceStyleTable::new();
        let bold = Style {
            attrs: Attrs::BOLD,
            ..Style::DEFAULT
        };
        assert_eq!(a.intern(Style::DEFAULT), StyleIdx::ZERO);
        assert_eq!(a.intern(bold), StyleIdx::new(1));
        assert_eq!(a.intern(rgb(7)), StyleIdx::new(2));
        assert_eq!(a.intern(bold), StyleIdx::new(1));
        assert_eq!(b.intern(bold), StyleIdx::new(1));
        assert_eq!(b.intern(rgb(7)), StyleIdx::new(2));
        assert_eq!(a.as_slice(), b.as_slice());
        assert_eq!(a.len(), 3);
    }

    #[test]
    fn new_styles_are_reported_once() {
        let mut t = SurfaceStyleTable::new();
        t.intern(rgb(1));
        t.intern(rgb(2));
        t.intern(rgb(1));
        assert!(t.has_new_styles());
        let first = t.take_new();
        assert_eq!(
            first,
            vec![(StyleIdx::new(1), rgb(1)), (StyleIdx::new(2), rgb(2))]
        );

        t.intern(rgb(1));
        assert!(!t.has_new_styles());
        assert!(
            t.take_new().is_empty(),
            "an already-flushed style is not resent"
        );

        t.intern(rgb(3));
        assert_eq!(t.take_new(), vec![(StyleIdx::new(3), rgb(3))]);
    }

    #[test]
    fn cap_triggers_reset_and_forces_a_snapshot() {
        let mut t = SurfaceStyleTable::new();
        for i in 1..STYLE_TABLE_CAP {
            assert_eq!(t.intern(rgb(i)), StyleIdx::new(i as u16));
        }
        assert!(t.is_full());
        assert_eq!(t.len(), STYLE_TABLE_CAP);
        assert!(!t.overflowed());
        // A style already in the table still resolves without a reset.
        assert_eq!(t.intern(rgb(1)), StyleIdx::new(1));
        assert!(!t.overflowed());

        // One more distinct style resets the table.
        let overflow = Style {
            attrs: Attrs::BLINK | Attrs::HIDDEN,
            ..Style::DEFAULT
        };
        assert_eq!(t.intern(overflow), StyleIdx::new(1));
        assert!(t.overflowed());
        assert_eq!(t.generation(), 1);
        assert_eq!(t.len(), 2);
        assert_eq!(t.get(StyleIdx::new(1)), Some(overflow));
        assert!(t.take_overflow());
        assert!(!t.take_overflow());
    }

    #[test]
    fn mark_all_flushed_after_a_snapshot() {
        let mut t = SurfaceStyleTable::new();
        t.intern(rgb(1));
        t.mark_all_flushed();
        assert!(t.new_styles().is_empty());
        t.intern(rgb(2));
        assert_eq!(t.new_styles().len(), 1);
    }

    #[test]
    fn rollback_resends_the_whole_table() {
        let mut t = SurfaceStyleTable::new();
        t.intern(rgb(1));
        t.take_new();
        t.intern(rgb(2));
        t.rollback_flush_window();
        assert_eq!(
            t.take_new().len(),
            2,
            "a discarded frame re-queues everything"
        );
    }
}
