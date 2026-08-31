//! The VT engine boundary (`docs/plan/03-server.md` §5).
//!
//! Everything above this module — packing, interning, publishing — is written
//! against the [`VtEngine`] trait and the plain data types declared here, so a
//! second engine (Ghostty, a mock, a replay harness) can be dropped in without
//! touching [`crate::surface`] or [`crate::publisher`]. Invariant I6: the only
//! file in the workspace that may name `alacritty_terminal` is
//! [`crate::vt::alacritty`].
//!
//! # Deviations from `03-server.md` §5
//!
//! * `snapshot()` returns a [`GridSnapshot`] rather than a
//!   `st_proto::Snapshot`: the engine does not know the Surface id, the
//!   sequence number, the View State or the process exit status, all of which
//!   the wire message carries. [`crate::surface::Surface`] assembles the two.
//! * `row()`/`history_lines()` return `st_proto::Row` directly instead of a
//!   separate `PackedRow` type — `st-proto` already owns that shape.
//! * The trait speaks [`crate::style_table::SurfaceStyleTable`] instead of a
//!   bare `st_proto::StyleTable` so that the grilling-Q45 overflow policy
//!   (reset the table, force a Snapshot) is applied wherever cells are packed.

pub mod alacritty;

use st_proto::{AbsLine, Cursor, Modes, Row};

use crate::style_table::SurfaceStyleTable;

/// A set of dirty visible-grid rows, one bit per row.
///
/// Row indices are viewport-relative: `0` is the top visible row. The set is
/// the only damage granularity the protocol has (grilling Q16), so column
/// bounds reported by an engine are deliberately dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirtySet {
    rows: usize,
    bits: Vec<u64>,
}

impl DirtySet {
    /// An empty set sized for `rows` visible rows.
    #[must_use]
    pub fn new(rows: usize) -> Self {
        Self {
            rows,
            bits: vec![0; rows.div_ceil(64)],
        }
    }

    /// Number of rows the set is sized for.
    #[inline]
    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Resizes the set to `rows`, dropping every bit.
    pub fn resize(&mut self, rows: usize) {
        self.rows = rows;
        self.bits.clear();
        self.bits.resize(rows.div_ceil(64), 0);
    }

    /// Marks row `row` dirty. Out-of-range indices are ignored.
    #[inline]
    pub fn set(&mut self, row: usize) {
        if row < self.rows {
            self.bits[row / 64] |= 1u64 << (row % 64);
        }
    }

    /// Marks every row dirty.
    pub fn set_all(&mut self) {
        for word in &mut self.bits {
            *word = u64::MAX;
        }
        self.mask_tail();
    }

    /// Drops every bit, keeping the size.
    pub fn clear(&mut self) {
        self.bits.iter_mut().for_each(|w| *w = 0);
    }

    /// Returns `true` when `row` is dirty.
    #[inline]
    #[must_use]
    pub fn contains(&self, row: usize) -> bool {
        row < self.rows && self.bits[row / 64] & (1u64 << (row % 64)) != 0
    }

    /// Returns `true` when no row is dirty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|w| *w == 0)
    }

    /// Number of dirty rows.
    #[must_use]
    pub fn count(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// ORs `other` into `self`, growing `self` if `other` is larger.
    ///
    /// A grown set keeps the bits it already had; the extra rows start clean.
    pub fn union_with(&mut self, other: &DirtySet) {
        if other.rows > self.rows {
            self.rows = other.rows;
            self.bits.resize(other.bits.len(), 0);
        }
        for (dst, src) in self.bits.iter_mut().zip(other.bits.iter()) {
            *dst |= *src;
        }
        self.mask_tail();
    }

    /// Iterates the dirty row indices in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.rows).filter(move |&r| self.contains(r))
    }

    fn mask_tail(&mut self) {
        let tail = self.rows % 64;
        if tail != 0 {
            if let Some(last) = self.bits.last_mut() {
                *last &= (1u64 << tail) - 1;
            }
        }
    }
}

/// What an engine reports as changed since the last [`VtEngine::take_damage`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Damage {
    /// Every visible row changed (alt-screen transition, resize, reset,
    /// scrolling with a non-zero display offset).
    Full,
    /// Exactly these rows changed.
    Rows(DirtySet),
}

impl Damage {
    /// `true` for [`Damage::Full`].
    #[inline]
    #[must_use]
    pub fn is_full(&self) -> bool {
        matches!(self, Damage::Full)
    }

    /// `true` when nothing at all changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Damage::Full => false,
            Damage::Rows(set) => set.is_empty(),
        }
    }

    /// Materialises the damage as a dirty set of `rows` bits.
    #[must_use]
    pub fn to_dirty_set(&self, rows: usize) -> DirtySet {
        let mut set = DirtySet::new(rows);
        match self {
            Damage::Full => set.set_all(),
            Damage::Rows(src) => set.union_with(src),
        }
        set
    }
}

/// A 24-bit colour, used only for OSC colour replies.
///
/// Cell colours travel as [`st_proto::Color`], which stays symbolic so the
/// Client's theme applies (grilling Q26/Q34).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

/// The pixel geometry a program asked for with a text-area size query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextAreaSize {
    /// Rows in the text area.
    pub rows: u16,
    /// Columns in the text area.
    pub cols: u16,
    /// Cell width in pixels; the Server does not know it and sends `0`.
    pub cell_width: u16,
    /// Cell height in pixels; the Server does not know it and sends `0`.
    pub cell_height: u16,
}

/// A deferred reply the Server must format and write back to the PTY.
///
/// The engine cannot answer OSC colour or size queries itself — it has no
/// theme and no font metrics — so it hands back the formatter the terminal
/// program expects (`03-server.md` §4, grilling Q48).
#[derive(Clone)]
pub struct ColorReply(std::sync::Arc<dyn Fn(Rgb) -> String + Send + Sync>);

impl ColorReply {
    /// Builds a reply formatter from a closure.
    #[must_use]
    pub fn new(f: std::sync::Arc<dyn Fn(Rgb) -> String + Send + Sync>) -> Self {
        Self(f)
    }

    /// Formats the escape sequence answering the query with `rgb`.
    #[must_use]
    pub fn format(&self, rgb: Rgb) -> String {
        (self.0)(rgb)
    }
}

impl std::fmt::Debug for ColorReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ColorReply(..)")
    }
}

/// Same as [`ColorReply`] for text-area size queries.
#[derive(Clone)]
pub struct SizeReply(std::sync::Arc<dyn Fn(TextAreaSize) -> String + Send + Sync>);

impl SizeReply {
    /// Builds a reply formatter from a closure.
    #[must_use]
    pub fn new(f: std::sync::Arc<dyn Fn(TextAreaSize) -> String + Send + Sync>) -> Self {
        Self(f)
    }

    /// Formats the escape sequence answering the query with `size`.
    #[must_use]
    pub fn format(&self, size: TextAreaSize) -> String {
        (self.0)(size)
    }
}

impl std::fmt::Debug for SizeReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SizeReply(..)")
    }
}

/// Something the terminal program asked for that the grid cannot express.
///
/// Drained after every [`VtEngine::advance`] (`03-server.md` §4).
#[derive(Debug, Clone)]
pub enum VtEvent {
    /// OSC 0/2: the window title changed.
    Title(String),
    /// OSC 0/2 with an empty payload: revert to the default title.
    ResetTitle,
    /// BEL.
    Bell,
    /// The program must receive these bytes on the PTY (DA, DSR, …).
    PtyWrite(Vec<u8>),
    /// OSC 52 store; only produced when clipboard support is enabled, which it
    /// is not in v1 (grilling Q48).
    ClipboardStore {
        /// The OSC 52 selection byte (`c`, `p`, …).
        kind: u8,
        /// The text the program wants stored.
        text: String,
    },
    /// OSC 4/10/11 colour query: the Server answers from `[theme]`.
    ColorRequest {
        /// Palette index, or 256/257 for the default fg/bg.
        index: usize,
        /// Formatter producing the escape sequence to write back.
        reply: ColorReply,
    },
    /// CSI 14 t / 16 t: the program asked for the text-area size.
    TextAreaSizeRequest {
        /// Formatter producing the escape sequence to write back.
        reply: SizeReply,
    },
}

/// A complete picture of the visible grid, ready to become a
/// [`st_proto::Snapshot`] once the Surface adds its id, `seq` and View State.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridSnapshot {
    /// Grid width.
    pub cols: u16,
    /// Grid height.
    pub rows: u16,
    /// Exactly `rows` entries, top to bottom, trailing blanks trimmed.
    pub grid: Vec<Row>,
    /// Cursor state.
    pub cursor: Cursor,
    /// Terminal modes.
    pub modes: Modes,
    /// Current window title.
    pub title: String,
    /// Id of the oldest retained history line.
    pub history_base: AbsLine,
    /// Number of retained history lines.
    pub history_len: u64,
}

/// The authoritative terminal state machine of one Surface (`03-server.md` §5).
///
/// Implementations are single-threaded but must be movable between threads,
/// hence `Send`.
pub trait VtEngine: Send {
    /// Feeds PTY output into the parser.
    fn advance(&mut self, bytes: &[u8]);

    /// Takes everything the program asked for since the last call.
    fn drain_events(&mut self) -> Vec<VtEvent>;

    /// Takes the damage accumulated since the last call and resets it.
    fn take_damage(&mut self) -> Damage;

    /// Renders the whole visible grid.
    fn snapshot(&self, styles: &mut SurfaceStyleTable) -> GridSnapshot;

    /// Renders one visible row, `0` being the top of the screen.
    fn row(&self, line: u16, styles: &mut SurfaceStyleTable) -> Row;

    /// The cursor and mode state a Delta or Snapshot carries.
    fn cursor_and_modes(&self) -> (Cursor, Modes);

    /// The current window title (OSC 0/2), or the default one.
    fn title(&self) -> &str;

    /// Resizes the grid. History reflow stays off (grilling Q40), so absolute
    /// line ids are never renumbered.
    fn resize(&mut self, cols: u16, rows: u16);

    /// Grid width in columns.
    fn cols(&self) -> u16;

    /// Grid height in rows.
    fn rows(&self) -> u16;

    /// Absolute id of the oldest retained history line.
    fn history_base(&self) -> AbsLine;

    /// Number of retained history lines, in [`AbsLine`] units.
    fn history_len(&self) -> u64;

    /// Renders up to `count` lines starting at absolute line `from`.
    ///
    /// The addressable range is `history_base ..= history_base + history_len +
    /// rows`; a request that starts below `history_base` is clamped up to it,
    /// and one that starts past the end returns an empty vector.
    fn history_lines(&self, from: AbsLine, count: u32, styles: &mut SurfaceStyleTable) -> Vec<Row>;

    /// Hard reset (RIS) — clears the grid, the history and the modes.
    fn reset(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_set_basics() {
        let mut set = DirtySet::new(70);
        assert!(set.is_empty());
        set.set(0);
        set.set(69);
        set.set(70); // out of range, ignored
        assert!(set.contains(0) && set.contains(69));
        assert!(!set.contains(1));
        assert_eq!(set.count(), 2);
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![0, 69]);
        set.clear();
        assert!(set.is_empty());
        set.set_all();
        assert_eq!(set.count(), 70);
    }

    #[test]
    fn dirty_set_union_grows() {
        let mut a = DirtySet::new(4);
        a.set(1);
        let mut b = DirtySet::new(100);
        b.set(99);
        a.union_with(&b);
        assert_eq!(a.rows(), 100);
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![1, 99]);
    }

    #[test]
    fn damage_to_dirty_set() {
        assert_eq!(Damage::Full.to_dirty_set(3).count(), 3);
        let mut set = DirtySet::new(3);
        set.set(2);
        assert_eq!(
            Damage::Rows(set).to_dirty_set(3).iter().collect::<Vec<_>>(),
            vec![2]
        );
        assert!(Damage::Rows(DirtySet::new(3)).is_empty());
        assert!(!Damage::Full.is_empty());
    }
}
