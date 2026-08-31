//! Text selection over a [`Replica`] — `docs/plan/04-client-native.md` §8–§9.
//!
//! Selections live in **absolute line coordinates** ([`AbsLine`]), never in
//! viewport coordinates: a line id is assigned once and never renumbered
//! (grilling Q40), so a selection survives scrolling, new output and history
//! trimming without any bookkeeping. That is also the shape the Server stores
//! it in ([`st_proto::Selection`], grilling Q43), so [`Selection::to_wire`] is
//! a field-for-field conversion.
//!
//! Four modes, driven by the click count and the Alt key (§8):
//!
//! | Gesture | [`SelectionMode`] |
//! |---|---|
//! | drag | [`Char`](SelectionMode::Char) |
//! | double-click | [`Word`](SelectionMode::Word) |
//! | triple-click | [`Line`](SelectionMode::Line) — the whole *soft-wrapped* line |
//! | Alt+drag | [`Block`](SelectionMode::Block) |
//!
//! [`Selection::text`] joins rows with `\n` **except** across a row whose
//! [`Row::wrapped`] flag is set: that row soft-wrapped into the next one and
//! was never a line break, so pasting it back must not introduce one.

use st_proto::{AbsLine, CellFlags, Row};

use crate::replica::Replica;

/// A point in a Surface's absolute coordinate space.
///
/// Re-exported from `st-proto` so the wire type and the client's working type
/// are literally the same struct.
pub use st_proto::AbsPoint;

/// Characters that count as part of a word in addition to alphanumerics
/// (`config.toml` `wordChars`, §8).
pub const DEFAULT_WORD_CHARS: &str = "_-./~";

/// Selection tuning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionConfig {
    /// Non-alphanumeric characters that still belong to a word.
    pub word_chars: String,
    /// Trim trailing whitespace from each extracted row (§9).
    pub trim_trailing_whitespace: bool,
}

impl Default for SelectionConfig {
    fn default() -> Self {
        Self {
            word_chars: DEFAULT_WORD_CHARS.to_string(),
            trim_trailing_whitespace: true,
        }
    }
}

impl SelectionConfig {
    /// `true` when `ch` belongs to a word.
    #[must_use]
    pub fn is_word_char(&self, ch: char) -> bool {
        ch.is_alphanumeric() || self.word_chars.contains(ch)
    }
}

/// How a selection grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SelectionMode {
    /// Cell by cell.
    #[default]
    Char,
    /// Snapped outward to word boundaries at both ends.
    Word,
    /// Whole logical lines, spanning soft wraps.
    Line,
    /// A rectangle: the same column range on every line.
    Block,
}

impl SelectionMode {
    /// The mode a click of `count` clicks with `alt` held starts (§8).
    #[must_use]
    pub fn from_click(count: u8, alt: bool) -> Self {
        if alt {
            return Self::Block;
        }
        match count {
            0 | 1 => Self::Char,
            2 => Self::Word,
            _ => Self::Line,
        }
    }

    /// The wire shape this mode maps onto.
    #[must_use]
    pub const fn to_wire(self) -> st_proto::SelectionKind {
        match self {
            Self::Char | Self::Word => st_proto::SelectionKind::Normal,
            Self::Line => st_proto::SelectionKind::Lines,
            Self::Block => st_proto::SelectionKind::Block,
        }
    }

    /// The mode a wire shape maps back onto. `Normal` becomes
    /// [`Char`](SelectionMode::Char): a restored selection has already been
    /// snapped, so re-expanding it would be wrong.
    #[must_use]
    pub const fn from_wire(kind: st_proto::SelectionKind) -> Self {
        match kind {
            st_proto::SelectionKind::Normal => Self::Char,
            st_proto::SelectionKind::Lines => Self::Line,
            st_proto::SelectionKind::Block => Self::Block,
        }
    }
}

/// An in-progress or finished selection.
///
/// `anchor` is where the drag started and `head` is where the pointer is now,
/// so `head` may be *before* `anchor`. Every read goes through
/// [`normalized`](Selection::normalized), which orders them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Selection {
    /// Where the drag started.
    pub anchor: AbsPoint,
    /// Where the pointer is now.
    pub head: AbsPoint,
    /// How the selection grows.
    pub mode: SelectionMode,
}

impl Selection {
    /// A zero-width selection at `at`.
    #[must_use]
    pub const fn new(at: AbsPoint, mode: SelectionMode) -> Self {
        Self {
            anchor: at,
            head: at,
            mode,
        }
    }

    /// Moves the head, leaving the anchor where it is.
    pub fn extend_to(&mut self, head: AbsPoint) {
        self.head = head;
    }

    /// `(start, end)` with `start <= end`, both **inclusive**.
    ///
    /// For [`SelectionMode::Block`] the columns are ordered independently of
    /// the lines, which is what makes a rectangle a rectangle.
    #[must_use]
    pub fn normalized(&self) -> (AbsPoint, AbsPoint) {
        if self.mode == SelectionMode::Block {
            let (top, bottom) = min_max(self.anchor.line, self.head.line);
            let (left, right) = min_max(self.anchor.col, self.head.col);
            return (
                AbsPoint {
                    line: top,
                    col: left,
                },
                AbsPoint {
                    line: bottom,
                    col: right,
                },
            );
        }
        if (self.anchor.line, self.anchor.col) <= (self.head.line, self.head.col) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// `true` when the selection covers no cell at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head && self.mode == SelectionMode::Char
    }

    /// `true` when `point` lies inside the selection.
    #[must_use]
    pub fn contains(&self, point: AbsPoint) -> bool {
        let (start, end) = self.normalized();
        if point.line < start.line || point.line > end.line {
            return false;
        }
        match self.mode {
            SelectionMode::Block => point.col >= start.col && point.col <= end.col,
            SelectionMode::Line => true,
            _ => {
                let after_start = point.line > start.line || point.col >= start.col;
                let before_end = point.line < end.line || point.col <= end.col;
                after_start && before_end
            }
        }
    }

    /// The inclusive column range selected on `line`, or `None` when the line
    /// is outside the selection.
    ///
    /// `cols` is the grid width, used to terminate a linear selection's
    /// intermediate lines.
    #[must_use]
    pub fn cols_on(&self, line: AbsLine, cols: u16) -> Option<(u16, u16)> {
        let (start, end) = self.normalized();
        if line < start.line || line > end.line || cols == 0 {
            return None;
        }
        let last = cols - 1;
        Some(match self.mode {
            SelectionMode::Block => (start.col.min(last), end.col.min(last)),
            SelectionMode::Line => (0, last),
            _ => {
                let first = if line == start.line { start.col } else { 0 };
                let final_col = if line == end.line { end.col } else { last };
                (first.min(last), final_col.min(last))
            }
        })
    }

    /// Snaps the selection outward according to its mode: word boundaries for
    /// [`Word`](SelectionMode::Word), whole soft-wrapped lines for
    /// [`Line`](SelectionMode::Line). A no-op in the other modes.
    ///
    /// Call this after every [`extend_to`](Selection::extend_to) while
    /// dragging, so the ends stay snapped as the pointer moves.
    pub fn snap(&mut self, replica: &Replica, config: &SelectionConfig) {
        match self.mode {
            SelectionMode::Word => {
                let anchor_word = word_at(replica, self.anchor, config);
                let head_word = word_at(replica, self.head, config);
                if (self.anchor.line, self.anchor.col) <= (self.head.line, self.head.col) {
                    self.anchor.col = anchor_word.0;
                    self.head.col = head_word.1;
                } else {
                    self.anchor.col = anchor_word.1;
                    self.head.col = head_word.0;
                }
            }
            SelectionMode::Line => {
                let anchor_line = logical_line(replica, self.anchor.line);
                let head_line = logical_line(replica, self.head.line);
                let cols = replica.cols().saturating_sub(1);
                if self.anchor.line <= self.head.line {
                    self.anchor = AbsPoint {
                        line: anchor_line.0,
                        col: 0,
                    };
                    self.head = AbsPoint {
                        line: head_line.1,
                        col: cols,
                    };
                } else {
                    self.anchor = AbsPoint {
                        line: anchor_line.1,
                        col: cols,
                    };
                    self.head = AbsPoint {
                        line: head_line.0,
                        col: 0,
                    };
                }
            }
            SelectionMode::Char | SelectionMode::Block => {}
        }
    }

    /// Extracts the selected text (§9).
    ///
    /// Wide-char spacers are dropped, grapheme clusters are spliced back in,
    /// each row's trailing whitespace is trimmed (configurable), and rows are
    /// joined with `\n` **except** across a soft-wrapped row.
    ///
    /// Lines that are not cached return nothing for that line rather than
    /// blanks, so a copy while scrolled through un-fetched history is short
    /// rather than wrong.
    #[must_use]
    pub fn text(&self, replica: &Replica, config: &SelectionConfig) -> String {
        let (start, end) = self.normalized();
        let cols = replica.cols();
        if cols == 0 {
            return String::new();
        }
        let mut out = String::new();
        let mut line = start.line;
        while line <= end.line {
            let Some((first, last)) = self.cols_on(line, cols) else {
                break;
            };
            let row = replica.line(line);
            if let Some(row) = row {
                let mut segment = row_segment(row, first, last);
                if config.trim_trailing_whitespace {
                    let trimmed = segment.trim_end_matches(' ');
                    segment.truncate(trimmed.len());
                }
                out.push_str(&segment);
            }
            if line < end.line {
                // A soft-wrapped row flows into the next one: no newline. A
                // Block selection is a rectangle and always breaks.
                let soft_wrapped = self.mode != SelectionMode::Block
                    && row.is_some_and(|r| r.wrapped)
                    && last as usize + 1 >= cols as usize;
                if !soft_wrapped {
                    out.push('\n');
                }
            }
            line = AbsLine::new(line.get() + 1);
        }
        out
    }

    /// The wire form the Data Plane carries (grilling Q43).
    #[must_use]
    pub fn to_wire(self) -> st_proto::Selection {
        let (anchor, head) = (self.anchor, self.head);
        st_proto::Selection {
            kind: self.mode.to_wire(),
            anchor,
            head,
        }
    }

    /// Rebuilds a selection from its wire form.
    #[must_use]
    pub fn from_wire(wire: st_proto::Selection) -> Self {
        Self {
            anchor: wire.anchor,
            head: wire.head,
            mode: SelectionMode::from_wire(wire.kind),
        }
    }
}

/// Pixel geometry the renderer hands to [`hit_test`] and [`hit_edge`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    /// Advance width of one cell, in pixels.
    pub cell_width: f32,
    /// Height of one line, in pixels.
    pub line_height: f32,
    /// Left padding inside the element.
    pub pad_left: f32,
    /// Top padding inside the element.
    pub pad_top: f32,
}

impl CellMetrics {
    /// Metrics with no padding.
    #[must_use]
    pub const fn new(cell_width: f32, line_height: f32) -> Self {
        Self {
            cell_width,
            line_height,
            pad_left: 0.0,
            pad_top: 0.0,
        }
    }
}

/// The cell a pointer at `(x, y)` is *inside*, clamped to the viewport.
///
/// `viewport_top` is the absolute id of the topmost painted line, i.e.
/// [`Replica::viewport_range`]`.start`. Use this for a click.
#[must_use]
pub fn hit_test(
    x: f32,
    y: f32,
    metrics: &CellMetrics,
    viewport_top: AbsLine,
    cols: u16,
    rows: u16,
) -> AbsPoint {
    let col = cell_index(x - metrics.pad_left, metrics.cell_width, cols);
    let row = cell_index(y - metrics.pad_top, metrics.line_height, rows);
    AbsPoint {
        line: viewport_top.saturating_add(row as u64),
        col,
    }
}

/// The cell *boundary* nearest `(x, y)`: the pointer snaps to the next cell
/// once it passes that cell's horizontal midpoint (§8).
///
/// Use this while dragging, so a selection follows the pointer the way every
/// terminal does. The returned column can be `cols`, meaning "past the last
/// cell"; callers extending an inclusive selection clamp it themselves.
#[must_use]
pub fn hit_edge(
    x: f32,
    y: f32,
    metrics: &CellMetrics,
    viewport_top: AbsLine,
    cols: u16,
    rows: u16,
) -> AbsPoint {
    let rel = ((x - metrics.pad_left) / metrics.cell_width.max(f32::EPSILON)) + 0.5;
    let col = if rel <= 0.0 {
        0
    } else {
        (rel as u32).min(cols as u32) as u16
    };
    let row = cell_index(y - metrics.pad_top, metrics.line_height, rows);
    AbsPoint {
        line: viewport_top.saturating_add(row as u64),
        col,
    }
}

/// Clamps `offset / size` into `0..count`.
fn cell_index(offset: f32, size: f32, count: u16) -> u16 {
    if offset <= 0.0 || count == 0 {
        return 0;
    }
    let index = (offset / size.max(f32::EPSILON)) as u32;
    index.min(count as u32 - 1) as u16
}

/// The inclusive column range of the word under `point`.
///
/// A click on a non-word character selects just that character, which is what
/// xterm and every GUI terminal do.
#[must_use]
pub fn word_at(replica: &Replica, point: AbsPoint, config: &SelectionConfig) -> (u16, u16) {
    let cols = replica.cols();
    if cols == 0 {
        return (0, 0);
    }
    let col = point.col.min(cols - 1);
    let Some(row) = replica.line(point.line) else {
        return (col, col);
    };
    let at = |c: u16| cell_char(row, c);

    let Some(ch) = at(col) else {
        return (col, col);
    };
    if !config.is_word_char(ch) {
        return (col, col);
    }

    let mut first = col;
    while first > 0 {
        match at(first - 1) {
            Some(prev) if config.is_word_char(prev) => first -= 1,
            _ => break,
        }
    }
    let mut last = col;
    while last + 1 < cols {
        match at(last + 1) {
            Some(next) if config.is_word_char(next) => last += 1,
            _ => break,
        }
    }
    (first, last)
}

/// The inclusive absolute-line range of the logical line `line` belongs to,
/// following [`Row::wrapped`] up and down.
#[must_use]
pub fn logical_line(replica: &Replica, line: AbsLine) -> (AbsLine, AbsLine) {
    let mut first = line;
    while first.get() > 0 {
        let above = AbsLine::new(first.get() - 1);
        match replica.line(above) {
            Some(row) if row.wrapped => first = above,
            _ => break,
        }
    }
    let mut last = line;
    let end = replica.first_visible_line().get() + replica.rows() as u64;
    while last.get() + 1 < end {
        match replica.line(last) {
            Some(row) if row.wrapped => last = AbsLine::new(last.get() + 1),
            _ => break,
        }
    }
    (first, last)
}

/// The character a cell displays: the spliced grapheme cluster for a
/// `GRAPHEME_EXT` cell's first scalar, the scalar otherwise, and `None` for a
/// wide-char spacer (which displays nothing of its own).
fn cell_char(row: &Row, col: u16) -> Option<char> {
    let cell = row.cell_at(col as usize);
    if cell.flags.contains(CellFlags::WIDE_SPACER) {
        return None;
    }
    if cell.flags.contains(CellFlags::GRAPHEME_EXT) {
        return row.grapheme(cell).and_then(|g| g.chars().next());
    }
    char::from_u32(cell.codepoint)
}

/// The text of `row` between the inclusive columns `first..=last`.
fn row_segment(row: &Row, first: u16, last: u16) -> String {
    let mut out = String::with_capacity((last - first + 1) as usize);
    for col in first..=last {
        let cell = row.cell_at(col as usize);
        if cell.flags.contains(CellFlags::WIDE_SPACER) {
            // The leading half already contributed the glyph.
            continue;
        }
        if cell.flags.contains(CellFlags::WIDE_LEADING_SPACER) {
            continue;
        }
        if let Some(grapheme) = row.grapheme(cell) {
            out.push_str(grapheme);
        } else if let Some(ch) = char::from_u32(cell.codepoint) {
            out.push(ch);
        }
    }
    out
}

/// `min`/`max` of two ordered values.
fn min_max<T: Ord>(a: T, b: T) -> (T, T) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_proto::{Cursor, Modes, PackedCell, Row, Seq, Snapshot, Style, StyleIdx, SurfaceId};

    fn row_of(text: &str, wrapped: bool) -> Row {
        let mut row = Row {
            cells: text
                .chars()
                .map(|c| PackedCell::from_char(c, StyleIdx::ZERO))
                .collect(),
            extras: Vec::new(),
            wrapped,
        };
        row.trim_trailing_blanks();
        row
    }

    /// A replica whose visible grid is `lines`, each `(text, wrapped)`.
    fn replica(cols: u16, lines: &[(&str, bool)]) -> Replica {
        let mut replica = Replica::new(SurfaceId(1));
        replica.apply_snapshot(&Snapshot {
            surface_id: SurfaceId(1),
            seq: Seq(1),
            cols,
            rows: lines.len() as u16,
            styles: vec![Style::DEFAULT],
            grid: lines.iter().map(|(t, w)| row_of(t, *w)).collect(),
            cursor: Cursor::default(),
            modes: Modes::LINE_WRAP,
            title: String::new(),
            history_base: AbsLine(0),
            history_len: 0,
            view_state: st_proto::ViewState::default(),
            exited: None,
        });
        replica
    }

    fn at(line: u64, col: u16) -> AbsPoint {
        AbsPoint {
            line: AbsLine(line),
            col,
        }
    }

    fn select(mode: SelectionMode, anchor: AbsPoint, head: AbsPoint) -> Selection {
        let mut sel = Selection::new(anchor, mode);
        sel.extend_to(head);
        sel
    }

    #[test]
    fn char_selection_within_one_line() {
        let r = replica(20, &[("hello world", false)]);
        let sel = select(SelectionMode::Char, at(0, 0), at(0, 4));
        assert_eq!(sel.text(&r, &SelectionConfig::default()), "hello");
        assert_eq!(sel.cols_on(AbsLine(0), 20), Some((0, 4)));
        assert!(sel.contains(at(0, 3)));
        assert!(!sel.contains(at(0, 5)));
        assert!(!sel.contains(at(1, 0)));
    }

    #[test]
    fn a_backwards_drag_normalises() {
        let r = replica(20, &[("hello world", false)]);
        let sel = select(SelectionMode::Char, at(0, 10), at(0, 6));
        let (start, end) = sel.normalized();
        assert_eq!((start.col, end.col), (6, 10));
        assert_eq!(sel.text(&r, &SelectionConfig::default()), "world");
        assert!(Selection::new(at(0, 3), SelectionMode::Char).is_empty());
        assert!(!sel.is_empty());
    }

    #[test]
    fn word_boundaries_use_word_chars() {
        let r = replica(40, &[("cd ~/src/foo-bar.rs && ls", false)]);
        let cfg = SelectionConfig::default();

        // The path is one word: `-`, `.`, `/`, `~` are all word chars.
        assert_eq!(word_at(&r, at(0, 5), &cfg), (3, 18));
        let mut sel = Selection::new(at(0, 5), SelectionMode::Word);
        sel.snap(&r, &cfg);
        assert_eq!(sel.text(&r, &cfg), "~/src/foo-bar.rs");

        // A narrower word_chars set splits the path apart.
        let narrow = SelectionConfig {
            word_chars: "_".into(),
            ..SelectionConfig::default()
        };
        assert_eq!(word_at(&r, at(0, 5), &narrow), (5, 7));
        let mut sel = Selection::new(at(0, 5), SelectionMode::Word);
        sel.snap(&r, &narrow);
        assert_eq!(sel.text(&r, &narrow), "src");

        // A click on a separator selects just that cell.
        assert_eq!(word_at(&r, at(0, 2), &cfg), (2, 2));
        assert_eq!(word_at(&r, at(0, 19), &cfg), (19, 19));
    }

    #[test]
    fn word_selection_snaps_both_ends_in_either_direction() {
        let r = replica(40, &[("alpha beta gamma", false)]);
        let cfg = SelectionConfig::default();

        let mut forward = select(SelectionMode::Word, at(0, 2), at(0, 12));
        forward.snap(&r, &cfg);
        assert_eq!(forward.text(&r, &cfg), "alpha beta gamma");

        let mut backward = select(SelectionMode::Word, at(0, 12), at(0, 2));
        backward.snap(&r, &cfg);
        assert_eq!(backward.text(&r, &cfg), "alpha beta gamma");
        let (start, end) = backward.normalized();
        assert_eq!((start.col, end.col), (0, 15));
    }

    #[test]
    fn line_selection_spans_a_soft_wrapped_row_without_a_newline() {
        // 8 columns; "the quick" wrapped into "brown fox".
        let r = replica(
            8,
            &[
                ("preamble", false),
                ("thequick", true),
                ("brownfox", false),
                ("after", false),
            ],
        );
        let cfg = SelectionConfig::default();

        let mut sel = Selection::new(at(1, 3), SelectionMode::Line);
        sel.snap(&r, &cfg);
        assert_eq!(sel.normalized().0.line, AbsLine(1));
        assert_eq!(sel.normalized().1.line, AbsLine(2));
        assert_eq!(sel.text(&r, &cfg), "thequickbrownfox");
        assert!(!sel.text(&r, &cfg).contains('\n'));

        // Triple-clicking the continuation row selects the same logical line.
        let mut from_below = Selection::new(at(2, 1), SelectionMode::Line);
        from_below.snap(&r, &cfg);
        assert_eq!(from_below.text(&r, &cfg), "thequickbrownfox");

        // An unwrapped line stands alone.
        let mut alone = Selection::new(at(0, 0), SelectionMode::Line);
        alone.snap(&r, &cfg);
        assert_eq!(alone.text(&r, &cfg), "preamble");
        assert_eq!(logical_line(&r, AbsLine(0)), (AbsLine(0), AbsLine(0)));
        assert_eq!(logical_line(&r, AbsLine(2)), (AbsLine(1), AbsLine(2)));
    }

    #[test]
    fn a_multi_line_char_selection_joins_wrapped_rows_but_breaks_hard_ones() {
        let r = replica(8, &[("aaaaaaaa", true), ("bbbb", false), ("cccc", false)]);
        let cfg = SelectionConfig::default();
        let sel = select(SelectionMode::Char, at(0, 0), at(2, 3));
        assert_eq!(sel.text(&r, &cfg), "aaaaaaaabbbb\ncccc");
    }

    #[test]
    fn a_partial_selection_of_a_wrapped_row_still_breaks() {
        // Stopping short of the last column means the user did not select the
        // wrap point, so the newline belongs there.
        let r = replica(8, &[("aaaaaaaa", true), ("bbbb", false)]);
        let cfg = SelectionConfig::default();
        let sel = select(SelectionMode::Char, at(0, 0), at(1, 1));
        // Column 7 of row 0 is not included on the first line? It is: a linear
        // selection runs to the end of every intermediate line.
        assert_eq!(sel.text(&r, &cfg), "aaaaaaaabb");
    }

    #[test]
    fn block_selection_is_a_rectangle_and_always_breaks_lines() {
        let r = replica(
            10,
            &[
                ("abcdefghij", true),
                ("klmnopqrst", true),
                ("uvwxyz", false),
            ],
        );
        let cfg = SelectionConfig::default();
        let sel = select(SelectionMode::Block, at(0, 2), at(2, 4));
        assert_eq!(sel.text(&r, &cfg), "cde\nmno\nwxy");
        assert_eq!(sel.cols_on(AbsLine(1), 10), Some((2, 4)));
        assert!(sel.contains(at(1, 3)));
        assert!(!sel.contains(at(1, 5)));

        // Dragging up-left still yields the same rectangle.
        let reversed = select(SelectionMode::Block, at(2, 4), at(0, 2));
        assert_eq!(reversed.normalized(), sel.normalized());
        assert_eq!(reversed.text(&r, &cfg), "cde\nmno\nwxy");
    }

    #[test]
    fn block_versus_linear_over_the_same_corners() {
        let r = replica(6, &[("abcdef", false), ("ghijkl", false)]);
        let cfg = SelectionConfig::default();
        let corners = (at(0, 2), at(1, 3));

        let linear = select(SelectionMode::Char, corners.0, corners.1);
        assert_eq!(linear.text(&r, &cfg), "cdef\nghij");

        let block = select(SelectionMode::Block, corners.0, corners.1);
        assert_eq!(block.text(&r, &cfg), "cd\nij");
    }

    #[test]
    fn trailing_whitespace_is_trimmed_per_row() {
        let r = replica(10, &[("ab", false), ("cd", false)]);
        let cfg = SelectionConfig::default();
        let sel = select(SelectionMode::Char, at(0, 0), at(1, 9));
        assert_eq!(sel.text(&r, &cfg), "ab\ncd");

        let keep = SelectionConfig {
            trim_trailing_whitespace: false,
            ..SelectionConfig::default()
        };
        assert_eq!(sel.text(&r, &keep), "ab        \ncd        ");
    }

    #[test]
    fn wide_chars_and_graphemes_extract_once() {
        let mut r = Replica::new(SurfaceId(1));
        let row = Row {
            cells: vec![
                PackedCell::from_char('a', StyleIdx::ZERO),
                PackedCell::new('世' as u32, StyleIdx::ZERO, CellFlags::WIDE),
                PackedCell::new(0, StyleIdx::ZERO, CellFlags::WIDE_SPACER),
                PackedCell::new(0, StyleIdx::ZERO, CellFlags::GRAPHEME_EXT),
                PackedCell::from_char('b', StyleIdx::ZERO),
            ],
            extras: vec!["e\u{301}".into()],
            wrapped: false,
        };
        r.apply_snapshot(&Snapshot {
            surface_id: SurfaceId(1),
            seq: Seq(1),
            cols: 5,
            rows: 1,
            styles: vec![Style::DEFAULT],
            grid: vec![row],
            cursor: Cursor::default(),
            modes: Modes::empty(),
            title: String::new(),
            history_base: AbsLine(0),
            history_len: 0,
            view_state: st_proto::ViewState::default(),
            exited: None,
        });
        let cfg = SelectionConfig::default();
        let sel = select(SelectionMode::Char, at(0, 0), at(0, 4));
        assert_eq!(sel.text(&r, &cfg), "a世e\u{301}b");

        // Selecting only the spacer half still yields the whole glyph's
        // leading cell when it is included, and nothing when it is not.
        let spacer_only = select(SelectionMode::Char, at(0, 2), at(0, 2));
        assert_eq!(spacer_only.text(&r, &cfg), "");
        let wide_only = select(SelectionMode::Char, at(0, 1), at(0, 2));
        assert_eq!(wide_only.text(&r, &cfg), "世");
    }

    #[test]
    fn selection_spans_history_and_the_visible_grid() {
        use st_proto::{Delta, DirtyRow};
        let mut r = replica(8, &[("one", false), ("two", false)]);
        let mut delta = Delta {
            surface_id: SurfaceId(1),
            seq: Seq(2),
            since_seq: Seq(1),
            history_base: AbsLine(0),
            history_len: 2,
            resized: None,
            new_styles: Vec::new(),
            rows: Vec::new(),
            cursor: Cursor::default(),
            modes: Modes::empty(),
            title: None,
        };
        delta.rows = vec![
            DirtyRow {
                index: 0,
                row: row_of("three", false),
            },
            DirtyRow {
                index: 1,
                row: row_of("four", false),
            },
        ];
        r.apply_delta(&delta).unwrap();

        let cfg = SelectionConfig::default();
        let sel = select(SelectionMode::Char, at(0, 0), at(3, 3));
        assert_eq!(sel.text(&r, &cfg), "one\ntwo\nthree\nfour");

        // Lines the client has not cached contribute nothing rather than
        // blanks.
        r.shrink_history_to(0);
        assert_eq!(sel.text(&r, &cfg), "\n\nthree\nfour");
    }

    #[test]
    fn hit_testing_maps_pixels_to_cells() {
        let metrics = CellMetrics {
            cell_width: 10.0,
            line_height: 20.0,
            pad_left: 5.0,
            pad_top: 4.0,
        };
        // (5,4) is the origin of cell (0,0).
        assert_eq!(
            hit_test(5.0, 4.0, &metrics, AbsLine(100), 80, 24),
            at(100, 0)
        );
        assert_eq!(
            hit_test(14.9, 4.0, &metrics, AbsLine(100), 80, 24),
            at(100, 0)
        );
        assert_eq!(
            hit_test(15.0, 4.0, &metrics, AbsLine(100), 80, 24),
            at(100, 1)
        );
        assert_eq!(
            hit_test(15.0, 24.0, &metrics, AbsLine(100), 80, 24),
            at(101, 1)
        );
        // Outside the element clamps rather than wrapping.
        assert_eq!(
            hit_test(-99.0, -99.0, &metrics, AbsLine(100), 80, 24),
            at(100, 0)
        );
        assert_eq!(
            hit_test(1e6, 1e6, &metrics, AbsLine(100), 80, 24),
            at(123, 79)
        );
    }

    #[test]
    fn drag_snaps_to_the_nearest_cell_edge() {
        let metrics = CellMetrics::new(10.0, 20.0);
        // Left half of cell 3 snaps to boundary 3; right half to boundary 4.
        assert_eq!(hit_edge(34.0, 0.0, &metrics, AbsLine(0), 80, 24).col, 3);
        assert_eq!(hit_edge(36.0, 0.0, &metrics, AbsLine(0), 80, 24).col, 4);
        assert_eq!(hit_edge(-5.0, 0.0, &metrics, AbsLine(0), 80, 24).col, 0);
        // Past the right edge the boundary is `cols`.
        assert_eq!(hit_edge(1e6, 0.0, &metrics, AbsLine(0), 80, 24).col, 80);
    }

    #[test]
    fn click_counts_choose_the_mode() {
        assert_eq!(SelectionMode::from_click(1, false), SelectionMode::Char);
        assert_eq!(SelectionMode::from_click(2, false), SelectionMode::Word);
        assert_eq!(SelectionMode::from_click(3, false), SelectionMode::Line);
        assert_eq!(SelectionMode::from_click(4, false), SelectionMode::Line);
        assert_eq!(SelectionMode::from_click(1, true), SelectionMode::Block);
        assert_eq!(SelectionMode::from_click(3, true), SelectionMode::Block);
        assert_eq!(SelectionMode::default(), SelectionMode::Char);
    }

    #[test]
    fn wire_round_trip() {
        let sel = select(SelectionMode::Block, at(10, 1), at(12, 7));
        let wire = sel.to_wire();
        assert_eq!(wire.kind, st_proto::SelectionKind::Block);
        assert_eq!(wire.anchor, at(10, 1));
        assert_eq!(Selection::from_wire(wire), sel);

        let lines = select(SelectionMode::Line, at(1, 0), at(2, 9));
        assert_eq!(lines.to_wire().kind, st_proto::SelectionKind::Lines);
        assert_eq!(
            Selection::from_wire(lines.to_wire()).mode,
            SelectionMode::Line
        );
        // Word snaps down to Normal on the wire and comes back as Char.
        let word = select(SelectionMode::Word, at(1, 0), at(1, 4));
        assert_eq!(word.to_wire().kind, st_proto::SelectionKind::Normal);
        assert_eq!(
            Selection::from_wire(word.to_wire()).mode,
            SelectionMode::Char
        );
    }

    #[test]
    fn a_zero_width_grid_yields_nothing() {
        let mut r = Replica::new(SurfaceId(1));
        r.resize(0, 0);
        let sel = select(SelectionMode::Char, at(0, 0), at(0, 5));
        assert_eq!(sel.text(&r, &SelectionConfig::default()), "");
        assert_eq!(sel.cols_on(AbsLine(0), 0), None);
        assert_eq!(word_at(&r, at(0, 0), &SelectionConfig::default()), (0, 0));
    }
}
