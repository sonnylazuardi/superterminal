//! Cell, row and style encoding — `docs/plan/02-protocol.md` §5.
//!
//! A cell is a [`PackedCell`]: a Unicode scalar (or an index into the row's
//! `extras` when [`CellFlags::GRAPHEME_EXT`] is set), a [`StyleIdx`] into the
//! Surface's [`StyleTable`], and one byte of [`CellFlags`]. In memory that is
//! 8 bytes; on the wire postcard varint-encodes the three fields, so a plain
//! ASCII cell with a small style index costs 3 bytes.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::StyleIdx;

bitflags::bitflags! {
    /// Per-cell layout flags (`02-protocol.md` §5.1).
    ///
    /// Bits 4–7 are reserved and must be zero in 1.x; they are masked off on
    /// decode so a newer peer's flags never break an older one.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct CellFlags: u8 {
        /// Leading cell of a two-column glyph.
        const WIDE = 1 << 0;
        /// Trailing half of a two-column glyph; `codepoint` is 0 and nothing is rendered.
        const WIDE_SPACER = 1 << 1;
        /// `codepoint` is an index into [`Row::extras`] rather than a Unicode scalar.
        const GRAPHEME_EXT = 1 << 2;
        /// Filler at the end of a row where a wide glyph did not fit and wrapped.
        const WIDE_LEADING_SPACER = 1 << 3;
    }
}

impl_flags_serde!(CellFlags, u8);

bitflags::bitflags! {
    /// Text attributes of a [`Style`] (`02-protocol.md` §5.3).
    ///
    /// The underline *kind* lives in bits 4–6 and is only meaningful when
    /// [`Attrs::UNDERLINE`] is set (`0` = single). Bits 11–15 are reserved and
    /// are masked off on decode.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct Attrs: u16 {
        /// Bold (SGR 1).
        const BOLD = 1 << 0;
        /// Dim / faint (SGR 2).
        const DIM = 1 << 1;
        /// Italic (SGR 3).
        const ITALIC = 1 << 2;
        /// Underlined (SGR 4); the kind is in bits 4–6, `0` meaning single.
        const UNDERLINE = 1 << 3;
        /// Underline kind: double.
        const UL_DOUBLE = 1 << 4;
        /// Underline kind: curly.
        const UL_CURLY = 2 << 4;
        /// Underline kind: dotted.
        const UL_DOTTED = 3 << 4;
        /// Underline kind: dashed.
        const UL_DASHED = 4 << 4;
        /// Strikethrough (SGR 9).
        const STRIKETHROUGH = 1 << 7;
        /// Inverse video (SGR 7).
        const INVERSE = 1 << 8;
        /// Hidden / concealed (SGR 8).
        const HIDDEN = 1 << 9;
        /// Blinking (SGR 5).
        const BLINK = 1 << 10;
    }
}

impl_flags_serde!(Attrs, u16);

impl Attrs {
    /// Mask covering the underline-kind field (bits 4–6).
    pub const UL_KIND_MASK: Attrs = Attrs::from_bits_truncate(0b111 << 4);

    /// Returns the underline-kind field as its small integer value
    /// (`0` = single, `1` = double, `2` = curly, `3` = dotted, `4` = dashed).
    ///
    /// The value is only meaningful when [`Attrs::UNDERLINE`] is set.
    #[inline]
    #[must_use]
    pub const fn underline_kind(self) -> u8 {
        ((self.bits() & Attrs::UL_KIND_MASK.bits()) >> 4) as u8
    }
}

/// A colour slot of a [`Style`] (`02-protocol.md` §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Color {
    /// The theme's default foreground/background/underline colour.
    #[default]
    Default,
    /// One of the 256 palette entries.
    Indexed(u8),
    /// A direct 24-bit colour.
    Rgb(u8, u8, u8),
}

/// One entry of a Surface's style table (`02-protocol.md` §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Style {
    /// Foreground colour.
    pub fg: Color,
    /// Background colour.
    pub bg: Color,
    /// Underline colour (SGR 58); [`Color::Default`] means "same as `fg`".
    pub underline_color: Color,
    /// Text attributes.
    pub attrs: Attrs,
}

impl Style {
    /// The default style, which is always index `0` of a [`StyleTable`].
    pub const DEFAULT: Style = Style {
        fg: Color::Default,
        bg: Color::Default,
        underline_color: Color::Default,
        attrs: Attrs::empty(),
    };
}

/// A single terminal cell (`02-protocol.md` §5.1).
///
/// `#[repr(C)]` with the fields in wire order: this is exactly 8 bytes in
/// memory (`u32 + u16 + u8 + 1 pad`), so a 200×60 replica grid is 96 KB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(C)]
pub struct PackedCell {
    /// Unicode scalar value, or an index into [`Row::extras`] when
    /// [`CellFlags::GRAPHEME_EXT`] is set.
    pub codepoint: u32,
    /// Index into the Surface's style table; `0` is the default style.
    pub style_idx: StyleIdx,
    /// Layout flags.
    pub flags: CellFlags,
}

impl PackedCell {
    /// A blank cell: a space in the default style with no flags.
    ///
    /// Trailing runs of this value are trimmed from [`Row::cells`] on the wire
    /// and re-padded by the receiver.
    pub const BLANK: Self = Self {
        codepoint: 0x20,
        style_idx: StyleIdx::ZERO,
        flags: CellFlags::empty(),
    };

    /// Builds a cell from a character and a style index.
    #[inline]
    #[must_use]
    pub const fn new(codepoint: u32, style_idx: StyleIdx, flags: CellFlags) -> Self {
        Self {
            codepoint,
            style_idx,
            flags,
        }
    }

    /// Builds a single-scalar cell in the given style.
    #[inline]
    #[must_use]
    pub const fn from_char(ch: char, style_idx: StyleIdx) -> Self {
        Self::new(ch as u32, style_idx, CellFlags::empty())
    }

    /// Returns `true` when this cell equals [`PackedCell::BLANK`].
    #[inline]
    #[must_use]
    pub fn is_blank(self) -> bool {
        self == Self::BLANK
    }

    /// Returns the [`Row::extras`] index when [`CellFlags::GRAPHEME_EXT`] is
    /// set, otherwise `None`.
    #[inline]
    #[must_use]
    pub fn grapheme_index(self) -> Option<usize> {
        self.flags
            .contains(CellFlags::GRAPHEME_EXT)
            .then_some(self.codepoint as usize)
    }

    /// Returns the cell as a single `u64`: `codepoint | style_idx << 32 | flags << 48`.
    ///
    /// This is a convenience for in-memory replica storage and tests. It is
    /// *not* the wire encoding — the wire encoding is postcard's varints over
    /// the three fields (§5.1).
    #[inline]
    #[must_use]
    pub const fn to_bits(self) -> u64 {
        (self.codepoint as u64)
            | ((self.style_idx.get() as u64) << 32)
            | ((self.flags.bits() as u64) << 48)
    }

    /// Inverse of [`PackedCell::to_bits`]. Reserved bits of the flags byte are
    /// masked off, and bits 56–63 are ignored.
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self {
            codepoint: bits as u32,
            style_idx: StyleIdx::new((bits >> 32) as u16),
            flags: CellFlags::from_bits_truncate((bits >> 48) as u8),
        }
    }
}

impl Default for PackedCell {
    fn default() -> Self {
        Self::BLANK
    }
}

/// One row of cells (`02-protocol.md` §4.4).
///
/// `cells.len() <= cols`: trailing [`PackedCell::BLANK`] cells are trimmed by
/// the sender and re-padded by the receiver (grilling Q41). `wrapped` marks a
/// soft-wrap continuation so copy/paste can join the row with the next one
/// without a newline.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Row {
    /// The row's cells, with trailing blanks trimmed.
    pub cells: Vec<PackedCell>,
    /// Multi-codepoint grapheme clusters referenced by cells carrying
    /// [`CellFlags::GRAPHEME_EXT`]; rebuilt every time the row is sent, so
    /// indices never dangle (§5.2).
    pub extras: Vec<String>,
    /// This row soft-wraps into the next one.
    pub wrapped: bool,
}

impl Row {
    /// An empty, unwrapped row.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cells: Vec::new(),
            extras: Vec::new(),
            wrapped: false,
        }
    }

    /// Removes trailing [`PackedCell::BLANK`] cells in place, as required
    /// before sending (§4.4).
    pub fn trim_trailing_blanks(&mut self) {
        while self.cells.last().is_some_and(|c| c.is_blank()) {
            self.cells.pop();
        }
    }

    /// Returns the cell at `col`, padding with [`PackedCell::BLANK`] past the
    /// trimmed tail. `col` is not bounds-checked against `cols`.
    #[inline]
    #[must_use]
    pub fn cell_at(&self, col: usize) -> PackedCell {
        self.cells.get(col).copied().unwrap_or(PackedCell::BLANK)
    }

    /// Pads the row with blanks so that `cells.len() == cols`, truncating if
    /// the row is longer.
    pub fn pad_to(&mut self, cols: usize) {
        self.cells.resize(cols, PackedCell::BLANK);
    }

    /// Returns the text of `cell`: the referenced grapheme cluster when
    /// [`CellFlags::GRAPHEME_EXT`] is set, otherwise `None` (the caller should
    /// use `char::from_u32(cell.codepoint)`).
    #[inline]
    #[must_use]
    pub fn grapheme(&self, cell: PackedCell) -> Option<&str> {
        let idx = cell.grapheme_index()?;
        self.extras.get(idx).map(String::as_str)
    }
}

/// Maximum number of entries in a Surface's style table (grilling Q45).
///
/// On overflow the server resets the table and forces a `Snapshot` to every
/// attached client; there is no compaction algorithm in v1.
pub const STYLE_TABLE_CAP: usize = 4096;

/// A Surface's interning style table (`02-protocol.md` §5.3).
///
/// Index `0` is always [`Style::DEFAULT`]. Entries are never freed within a
/// Surface's lifetime; when the table is full, [`StyleTable::intern`] returns
/// `None` and the caller resets the table and re-snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleTable {
    styles: Vec<Style>,
    index: HashMap<Style, StyleIdx>,
}

impl Default for StyleTable {
    fn default() -> Self {
        Self::new()
    }
}

impl StyleTable {
    /// Creates a table holding only [`Style::DEFAULT`] at index `0`.
    #[must_use]
    pub fn new() -> Self {
        let mut table = Self {
            styles: Vec::with_capacity(16),
            index: HashMap::with_capacity(16),
        };
        table.styles.push(Style::DEFAULT);
        table.index.insert(Style::DEFAULT, StyleIdx::ZERO);
        table
    }

    /// Rebuilds a table from a wire-order slice (a `Snapshot`'s `styles`).
    ///
    /// Duplicates keep their *first* index, which is the index the sender
    /// would have used. Returns `None` if the slice is empty, does not start
    /// with [`Style::DEFAULT`], or exceeds [`STYLE_TABLE_CAP`].
    #[must_use]
    pub fn from_wire(styles: &[Style]) -> Option<Self> {
        if styles.first() != Some(&Style::DEFAULT) || styles.len() > STYLE_TABLE_CAP {
            return None;
        }
        let mut index = HashMap::with_capacity(styles.len());
        for (i, style) in styles.iter().enumerate() {
            index.entry(*style).or_insert(StyleIdx::new(i as u16));
        }
        Some(Self {
            styles: styles.to_vec(),
            index,
        })
    }

    /// Returns the index of `style`, assigning the next free one if it is new.
    ///
    /// Returns `None` when the table already holds [`STYLE_TABLE_CAP`] entries
    /// and `style` is not among them — the caller must then
    /// [`reset`](StyleTable::reset) and send a fresh `Snapshot` (grilling Q45).
    pub fn intern(&mut self, style: Style) -> Option<StyleIdx> {
        if let Some(&idx) = self.index.get(&style) {
            return Some(idx);
        }
        if self.styles.len() >= STYLE_TABLE_CAP {
            return None;
        }
        let idx = StyleIdx::new(self.styles.len() as u16);
        self.styles.push(style);
        self.index.insert(style, idx);
        Some(idx)
    }

    /// Inserts `style` at `idx`, growing the table with [`Style::DEFAULT`] if
    /// needed. Used by the client to apply `Delta.new_styles`.
    ///
    /// Returns `false` (and changes nothing) if `idx` is at or beyond
    /// [`STYLE_TABLE_CAP`].
    pub fn set(&mut self, idx: StyleIdx, style: Style) -> bool {
        let i = idx.get() as usize;
        if i >= STYLE_TABLE_CAP {
            return false;
        }
        if i >= self.styles.len() {
            self.styles.resize(i + 1, Style::DEFAULT);
        }
        self.styles[i] = style;
        self.index.entry(style).or_insert(idx);
        true
    }

    /// Returns the style at `idx`, or `None` if the index is not populated.
    #[inline]
    #[must_use]
    pub fn get(&self, idx: StyleIdx) -> Option<Style> {
        self.styles.get(idx.get() as usize).copied()
    }

    /// Returns the style at `idx`, falling back to [`Style::DEFAULT`] for an
    /// unpopulated index (what a renderer wants).
    #[inline]
    #[must_use]
    pub fn get_or_default(&self, idx: StyleIdx) -> Style {
        self.get(idx).unwrap_or(Style::DEFAULT)
    }

    /// The table in wire order, as carried by `Snapshot.styles`.
    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[Style] {
        &self.styles
    }

    /// Number of entries, always at least 1.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.styles.len()
    }

    /// Always `false`: the table always holds [`Style::DEFAULT`].
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Returns `true` when the table cannot accept any further new style.
    #[inline]
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.styles.len() >= STYLE_TABLE_CAP
    }

    /// Drops every entry but [`Style::DEFAULT`] (the Q45 overflow path).
    pub fn reset(&mut self) {
        self.styles.clear();
        self.index.clear();
        self.styles.push(Style::DEFAULT);
        self.index.insert(Style::DEFAULT, StyleIdx::ZERO);
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Color::Default => f.write_str("default"),
            Color::Indexed(i) => write!(f, "idx({i})"),
            Color::Rgb(r, g, b) => write!(f, "#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_cell_is_eight_bytes() {
        assert_eq!(std::mem::size_of::<PackedCell>(), 8);
        assert_eq!(std::mem::align_of::<PackedCell>(), 4);
    }

    #[test]
    fn packed_cell_bit_round_trip() {
        let cell = PackedCell::new(0x1F600, StyleIdx::new(0xBEEF), CellFlags::WIDE);
        assert_eq!(PackedCell::from_bits(cell.to_bits()), cell);
        assert_eq!(
            PackedCell::from_bits(PackedCell::BLANK.to_bits()),
            PackedCell::BLANK
        );
    }

    #[test]
    fn from_bits_masks_reserved_flag_bits() {
        let raw = PackedCell::BLANK.to_bits() | (0xF0u64 << 48);
        assert_eq!(PackedCell::from_bits(raw).flags, CellFlags::empty());
    }

    #[test]
    fn ascii_cell_is_three_bytes_on_the_wire() {
        let cell = PackedCell::from_char('a', StyleIdx::new(3));
        assert_eq!(postcard::to_stdvec(&cell).unwrap().len(), 3);
    }

    #[test]
    fn flags_decode_truncates_unknown_bits() {
        let bytes = postcard::to_stdvec(&0xFFu8).unwrap();
        let flags: CellFlags = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(flags, CellFlags::all());

        let bytes = postcard::to_stdvec(&0xFFFFu16).unwrap();
        let attrs: Attrs = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(attrs, Attrs::all());
        assert!(!attrs.contains(Attrs::from_bits_retain(1 << 11)));
    }

    #[test]
    fn underline_kind_extraction() {
        assert_eq!((Attrs::UNDERLINE | Attrs::UL_CURLY).underline_kind(), 2);
        assert_eq!(Attrs::UNDERLINE.underline_kind(), 0);
        assert_eq!((Attrs::UNDERLINE | Attrs::UL_DASHED).underline_kind(), 4);
    }

    #[test]
    fn row_trimming_and_padding() {
        let mut row = Row::new();
        row.cells = vec![
            PackedCell::from_char('h', StyleIdx::ZERO),
            PackedCell::BLANK,
            PackedCell::BLANK,
        ];
        row.trim_trailing_blanks();
        assert_eq!(row.cells.len(), 1);
        assert_eq!(row.cell_at(7), PackedCell::BLANK);
        row.pad_to(4);
        assert_eq!(row.cells.len(), 4);
        assert_eq!(row.cells[3], PackedCell::BLANK);
    }

    #[test]
    fn row_grapheme_lookup() {
        let mut row = Row::new();
        row.extras.push("e\u{301}".to_string());
        let cell = PackedCell::new(0, StyleIdx::ZERO, CellFlags::GRAPHEME_EXT);
        row.cells.push(cell);
        assert_eq!(row.grapheme(cell), Some("e\u{301}"));
        assert_eq!(row.grapheme(PackedCell::BLANK), None);
    }

    #[test]
    fn interning_is_deterministic_and_stable() {
        let mut table = StyleTable::new();
        assert_eq!(table.get(StyleIdx::ZERO), Some(Style::DEFAULT));

        let bold = Style {
            attrs: Attrs::BOLD,
            ..Style::DEFAULT
        };
        let red = Style {
            fg: Color::Indexed(1),
            ..Style::DEFAULT
        };
        assert_eq!(table.intern(bold), Some(StyleIdx::new(1)));
        assert_eq!(table.intern(red), Some(StyleIdx::new(2)));
        assert_eq!(table.intern(bold), Some(StyleIdx::new(1)));
        assert_eq!(table.intern(Style::DEFAULT), Some(StyleIdx::ZERO));
        assert_eq!(table.len(), 3);

        let mut other = StyleTable::new();
        assert_eq!(other.intern(bold), Some(StyleIdx::new(1)));
        assert_eq!(other.intern(red), Some(StyleIdx::new(2)));
        assert_eq!(other.as_slice(), table.as_slice());
    }

    #[test]
    fn interning_stops_at_the_cap() {
        let mut table = StyleTable::new();
        for i in 1..STYLE_TABLE_CAP {
            let style = Style {
                fg: Color::Rgb((i >> 16) as u8, (i >> 8) as u8, i as u8),
                ..Style::DEFAULT
            };
            assert_eq!(table.intern(style), Some(StyleIdx::new(i as u16)));
        }
        assert!(table.is_full());
        assert_eq!(table.len(), STYLE_TABLE_CAP);

        // A style already present still resolves at the cap.
        assert_eq!(table.intern(Style::DEFAULT), Some(StyleIdx::ZERO));
        // A new one does not.
        let overflow = Style {
            attrs: Attrs::BLINK | Attrs::HIDDEN,
            ..Style::DEFAULT
        };
        assert_eq!(table.intern(overflow), None);

        table.reset();
        assert_eq!(table.len(), 1);
        assert!(!table.is_full());
        assert_eq!(table.intern(overflow), Some(StyleIdx::new(1)));
    }

    #[test]
    fn from_wire_rejects_bad_tables() {
        assert!(StyleTable::from_wire(&[]).is_none());
        assert!(StyleTable::from_wire(&[Style {
            attrs: Attrs::BOLD,
            ..Style::DEFAULT
        }])
        .is_none());
        let table = StyleTable::from_wire(&[Style::DEFAULT, Style::DEFAULT]).unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(StyleIdx::new(1)), Some(Style::DEFAULT));
    }

    #[test]
    fn set_grows_and_rejects_out_of_cap() {
        let mut table = StyleTable::new();
        let s = Style {
            bg: Color::Rgb(1, 2, 3),
            ..Style::DEFAULT
        };
        assert!(table.set(StyleIdx::new(4), s));
        assert_eq!(table.len(), 5);
        assert_eq!(table.get(StyleIdx::new(4)), Some(s));
        assert_eq!(table.get_or_default(StyleIdx::new(2)), Style::DEFAULT);
        assert!(!table.set(StyleIdx::new(STYLE_TABLE_CAP as u16), s));
    }

    #[test]
    fn color_display() {
        assert_eq!(Color::Default.to_string(), "default");
        assert_eq!(Color::Indexed(9).to_string(), "idx(9)");
        assert_eq!(Color::Rgb(0x11, 0x22, 0x33).to_string(), "#112233");
    }
}
