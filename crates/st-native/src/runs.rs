//! Turning one Replica row into paintable spans (04 §6 steps 2–3, 7, 8).
//!
//! Two passes over the row produce (a) merged background quads and (b) runs of
//! cells that share a text style, which is the unit the shaped-line cache is
//! keyed on. Neither pass touches gpui, so both are unit-testable headless;
//! `paint.rs` is the thin layer that turns the output into draw calls.

use std::collections::HashMap;
use std::hash::Hash;

use st_client_core::palette::{Palette, Rgb};
use st_proto::{Attrs, CellFlags, Row, StyleTable};

/// Everything about a cell that changes how its glyphs are shaped or coloured.
///
/// Deliberately *not* the whole `Style`: background is handled by the quad
/// pass, and `INVERSE`/`DIM`/`HIDDEN`/`bold_is_bright` have already been folded
/// into `fg` by [`Palette::resolve_style`], so two cells that differ only in
/// how they arrived at the same colour still share a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StyleKey {
    /// Glyph colour after inverse, selection, dim and hidden.
    pub fg: Rgb,
    /// Underline colour (SGR 58), defaulting to `fg`.
    pub underline_color: Rgb,
    /// Synthetic-bold / heavier face.
    pub bold: bool,
    /// Italic face.
    pub italic: bool,
    /// `0` none, `1` single, `2` double, `3` curly, `4` dotted, `5` dashed.
    ///
    /// `st_proto` splits this in two — an `UNDERLINE` flag and a kind field
    /// that is only meaningful when the flag is set — and folding them into
    /// one number here keeps "no underline" out of the `Option` dance at every
    /// use site.
    pub underline: u8,
    /// Strikethrough.
    pub strike: bool,
}

impl StyleKey {
    /// The key for a resolved style.
    #[must_use]
    pub fn from_resolved(style: &st_client_core::palette::ResolvedStyle) -> Self {
        Self {
            fg: style.fg,
            underline_color: style.underline,
            bold: style.attrs.contains(Attrs::BOLD),
            italic: style.attrs.contains(Attrs::ITALIC),
            underline: if style.attrs.contains(Attrs::UNDERLINE) {
                1 + style.attrs.underline_kind()
            } else {
                0
            },
            strike: style.attrs.contains(Attrs::STRIKETHROUGH),
        }
    }

    /// `true` when the run paints nothing but glyph shapes — no decoration.
    /// A run of spaces in such a style can be dropped entirely.
    #[must_use]
    pub fn is_plain(&self) -> bool {
        self.underline == 0 && !self.strike
    }
}

/// A merged span of cells sharing one background colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BgSpan {
    /// First column.
    pub col: u16,
    /// Number of cells covered.
    pub cells: u16,
    /// Fill colour.
    pub color: Rgb,
}

/// A run of adjacent cells that share a [`StyleKey`], with its text spliced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpan {
    /// First column.
    pub col: u16,
    /// Cells covered — two for a wide character.
    pub cells: u16,
    /// The text to shape.
    pub text: String,
    /// Colour and decoration.
    pub key: StyleKey,
    /// This run is a single double-width character.
    ///
    /// gpui's `force_width` snaps every *base glyph* to `n × force_width`, so
    /// a wide char inside a run would push everything after it one column to
    /// the left. Wide cells therefore get a run of their own, painted without
    /// a forced advance and clipped to two cells by the content mask
    /// (04 §6 step 7).
    pub wide: bool,
}

/// One row's worth of paint work.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowLayout {
    /// Background quads, left to right, already merged.
    pub backgrounds: Vec<BgSpan>,
    /// Glyph runs, left to right.
    pub runs: Vec<RunSpan>,
}

/// Lays out one row.
///
/// * `selected` is the inclusive column range the selection covers on this
///   line, as [`st_client_core::Selection::cols_on`] reports it.
/// * Cells whose background is the theme default get no quad at all, so a
///   blurred or transparent window background shows through (grilling Q28).
/// * `WIDE_SPACER` cells contribute neither glyph nor quad: the wide cell
///   before them already covers two columns.
#[must_use]
pub fn layout_row(
    row: &Row,
    cols: u16,
    styles: &StyleTable,
    palette: &Palette,
    selected: Option<(u16, u16)>,
) -> RowLayout {
    let mut layout = RowLayout::default();
    let mut col = 0u16;
    while col < cols {
        let cell = row.cell_at(usize::from(col));
        if cell.flags.contains(CellFlags::WIDE_SPACER) {
            col += 1;
            continue;
        }

        let is_selected = selected.is_some_and(|(a, b)| col >= a && col <= b);
        let resolved = palette.resolve_style(styles.get_or_default(cell.style_idx), is_selected);
        let width = if cell.flags.contains(CellFlags::WIDE) {
            2u16.min(cols - col)
        } else {
            1
        };

        if !resolved.bg_is_default {
            push_bg(&mut layout.backgrounds, col, width, resolved.bg);
        }

        let key = StyleKey::from_resolved(&resolved);
        let text = cell_text(row, cell);
        let wide = cell.flags.contains(CellFlags::WIDE);
        push_run(&mut layout.runs, col, width, &text, key, wide);

        col += width;
    }

    layout.runs.retain(|run| !is_droppable(run));
    trim_trailing_blanks(&mut layout.runs);
    layout
}

/// Drops the run of padding spaces every row ends in.
///
/// `Row::cell_at` pads past the trimmed wire row (Q41), so without this a
/// two-character prompt would shape an 80-character line every frame and blow
/// a cache entry per column the cursor moves.
fn trim_trailing_blanks(runs: &mut Vec<RunSpan>) {
    while let Some(last) = runs.last_mut() {
        if !last.key.is_plain() || last.wide {
            return;
        }
        let kept = last.text.trim_end_matches(' ').len();
        if kept == last.text.len() {
            return;
        }
        let dropped = last.text[kept..].chars().count();
        if kept == 0 {
            runs.pop();
            continue;
        }
        last.text.truncate(kept);
        last.cells -= u16::try_from(dropped).unwrap_or(last.cells);
        return;
    }
}

/// The text one cell contributes: a grapheme cluster from the row's side
/// table, the codepoint, or a space when the codepoint is not a character.
fn cell_text(row: &Row, cell: st_proto::PackedCell) -> String {
    if cell.flags.contains(CellFlags::GRAPHEME_EXT) {
        return row.grapheme(cell).unwrap_or(" ").to_string();
    }
    // A zero codepoint is an unwritten cell, and `WIDE_LEADING_SPACER` is the
    // filler a wide char wrapped from the previous line leaves behind.
    if cell.codepoint == 0 || cell.flags.contains(CellFlags::WIDE_LEADING_SPACER) {
        return " ".to_string();
    }
    char::from_u32(cell.codepoint)
        .filter(|ch| !ch.is_control())
        .map_or_else(|| " ".to_string(), String::from)
}

/// Appends a background, merging with the previous span when it is adjacent
/// and the same colour.
fn push_bg(spans: &mut Vec<BgSpan>, col: u16, cells: u16, color: Rgb) {
    if let Some(last) = spans.last_mut() {
        if last.color == color && last.col + last.cells == col {
            last.cells += cells;
            return;
        }
    }
    spans.push(BgSpan { col, cells, color });
}

/// Appends a run, merging with the previous one when it is adjacent and shares
/// the style key.
fn push_run(runs: &mut Vec<RunSpan>, col: u16, cells: u16, text: &str, key: StyleKey, wide: bool) {
    if !wide {
        if let Some(last) = runs.last_mut() {
            if !last.wide && last.key == key && last.col + last.cells == col {
                last.cells += cells;
                last.text.push_str(text);
                return;
            }
        }
    }
    runs.push(RunSpan {
        col,
        cells,
        text: text.to_string(),
        key,
        wide,
    });
}

/// A run of nothing but spaces in an undecorated style paints no pixels.
fn is_droppable(run: &RunSpan) -> bool {
    run.key.is_plain() && run.text.chars().all(|ch| ch == ' ')
}

/// Lays out the `rows` lines starting at absolute line `top`.
///
/// Lines the Replica has not cached yet come out as an empty [`RowLayout`], so
/// the viewport paints blank while `FetchHistory` is in flight (Q25) instead of
/// stalling the frame.
#[must_use]
pub fn layout_viewport(
    replica: &st_client_core::Replica,
    cols: u16,
    rows: u16,
    top: st_proto::AbsLine,
    selection: Option<&st_client_core::Selection>,
    palette: &Palette,
) -> Vec<RowLayout> {
    let mut out = Vec::with_capacity(usize::from(rows));
    for index in 0..u64::from(rows) {
        let line = st_proto::AbsLine::new(top.get() + index);
        let selected = selection.and_then(|selection| selection.cols_on(line, cols));
        match replica.line(line) {
            Some(row) => out.push(layout_row(row, cols, replica.styles(), palette, selected)),
            None => out.push(RowLayout::default()),
        }
    }
    out
}

/// A tiny LRU keyed on `(text, style)` (04 §6 step 3).
///
/// Not `lru`-the-crate: the working set is one screen of runs, capacity is in
/// the low hundreds, and the eviction scan is amortised over a whole batch, so
/// the dependency would buy nothing. Generic over the value so the policy can
/// be tested without a GPU-backed `ShapedLine`.
pub struct RunCache<K, V> {
    entries: HashMap<K, Entry<V>>,
    capacity: usize,
    clock: u64,
    hits: u64,
    misses: u64,
}

struct Entry<V> {
    value: V,
    used_at: u64,
}

impl<K: Eq + Hash + Clone, V> RunCache<K, V> {
    /// A cache holding at most `capacity` entries (04 §6 suggests 2× rows).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity: capacity.max(1),
            clock: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// Resizes, evicting immediately if the new capacity is smaller.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        self.evict();
    }

    /// The cached value, counting a hit or a miss either way.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.clock += 1;
        let clock = self.clock;
        match self.entries.get_mut(key) {
            Some(entry) => {
                entry.used_at = clock;
                self.hits += 1;
                Some(&entry.value)
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    /// Inserts, evicting the least recently used entries when over capacity.
    pub fn insert(&mut self, key: K, value: V) {
        self.clock += 1;
        self.entries.insert(
            key,
            Entry {
                value,
                used_at: self.clock,
            },
        );
        self.evict();
    }

    /// Drops everything. Called on a font or palette change, both of which
    /// invalidate every key (04 §6).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Live entry count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when nothing is cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Lifetime hit and miss counts, for the `stats` read-back (04-OQ10).
    #[must_use]
    pub fn counters(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// Lifetime hit rate in `0.0..=1.0`; `0.0` before the first lookup.
    #[must_use]
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }

    /// Forgets the hit/miss counters without dropping the cached lines, so a
    /// benchmark can measure one scenario at a time.
    pub fn reset_counters(&mut self) {
        self.hits = 0;
        self.misses = 0;
    }

    /// Drops the oldest quarter once the cache overflows: one scan instead of
    /// a scan per insert while a screen of new runs streams in.
    fn evict(&mut self) {
        if self.entries.len() <= self.capacity {
            return;
        }
        let target = self.capacity - self.capacity / 4;
        let mut ages: Vec<u64> = self.entries.values().map(|entry| entry.used_at).collect();
        ages.sort_unstable();
        let cutoff = ages[self.entries.len() - target];
        self.entries.retain(|_, entry| entry.used_at >= cutoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_client_core::palette::Palette;
    use st_client_core::Selection;
    use st_proto::{Color, PackedCell, Style, StyleIdx};

    fn table(styles: &[Style]) -> StyleTable {
        let mut table = StyleTable::new();
        for style in styles {
            table.intern(*style);
        }
        table
    }

    fn text_row(text: &str, style_idx: u16) -> Row {
        let mut row = Row::new();
        for ch in text.chars() {
            row.cells
                .push(PackedCell::from_char(ch, StyleIdx::new(style_idx)));
        }
        row
    }

    fn palette() -> Palette {
        crate::theme::default_palette()
    }

    #[test]
    fn a_plain_row_is_one_run_and_no_background() {
        let row = text_row("hello", 0);
        let layout = layout_row(&row, 5, &table(&[]), &palette(), None);
        assert!(layout.backgrounds.is_empty(), "{:?}", layout.backgrounds);
        assert_eq!(layout.runs.len(), 1);
        assert_eq!(layout.runs[0].text, "hello");
        assert_eq!(layout.runs[0].cells, 5);
        assert_eq!(layout.runs[0].col, 0);
    }

    #[test]
    fn trailing_blanks_are_padded_but_not_painted() {
        let row = text_row("hi", 0);
        let layout = layout_row(&row, 80, &table(&[]), &palette(), None);
        assert_eq!(layout.runs.len(), 1);
        assert_eq!(layout.runs[0].text, "hi");
    }

    #[test]
    fn adjacent_cells_with_the_same_style_merge_into_one_run() {
        let red = Style {
            fg: Color::Indexed(1),
            ..Style::DEFAULT
        };
        let mut row = Row::new();
        for ch in "aaa".chars() {
            row.cells.push(PackedCell::from_char(ch, StyleIdx::new(1)));
        }
        for ch in "bbb".chars() {
            row.cells.push(PackedCell::from_char(ch, StyleIdx::new(0)));
        }
        let layout = layout_row(&row, 6, &table(&[red]), &palette(), None);
        assert_eq!(layout.runs.len(), 2);
        assert_eq!(layout.runs[0].text, "aaa");
        assert_eq!(layout.runs[1].text, "bbb");
        assert_eq!(layout.runs[1].col, 3);
    }

    #[test]
    fn adjacent_backgrounds_of_the_same_colour_merge_into_one_quad() {
        let on_blue = Style {
            bg: Color::Indexed(4),
            ..Style::DEFAULT
        };
        let mut row = Row::new();
        for ch in "1234".chars() {
            row.cells.push(PackedCell::from_char(ch, StyleIdx::new(1)));
        }
        let layout = layout_row(&row, 10, &table(&[on_blue]), &palette(), None);
        assert_eq!(layout.backgrounds.len(), 1);
        assert_eq!(layout.backgrounds[0].col, 0);
        assert_eq!(layout.backgrounds[0].cells, 4);
    }

    #[test]
    fn two_different_backgrounds_do_not_merge() {
        let blue = Style {
            bg: Color::Indexed(4),
            ..Style::DEFAULT
        };
        let green = Style {
            bg: Color::Indexed(2),
            ..Style::DEFAULT
        };
        let mut row = Row::new();
        row.cells.push(PackedCell::from_char('a', StyleIdx::new(1)));
        row.cells.push(PackedCell::from_char('b', StyleIdx::new(2)));
        row.cells.push(PackedCell::from_char('c', StyleIdx::new(1)));
        let layout = layout_row(&row, 3, &table(&[blue, green]), &palette(), None);
        assert_eq!(layout.backgrounds.len(), 3);
        assert_eq!(layout.backgrounds[2].col, 2);
    }

    #[test]
    fn a_wide_char_covers_two_cells_and_its_spacer_paints_nothing() {
        let mut row = Row::new();
        row.cells.push(PackedCell::new(
            '世' as u32,
            StyleIdx::new(0),
            CellFlags::WIDE,
        ));
        row.cells
            .push(PackedCell::new(0, StyleIdx::new(0), CellFlags::WIDE_SPACER));
        row.cells.push(PackedCell::from_char('x', StyleIdx::new(0)));
        let layout = layout_row(&row, 3, &table(&[]), &palette(), None);
        assert_eq!(layout.runs.len(), 2, "a wide char never shares a run");
        assert_eq!(layout.runs[0].text, "世");
        assert_eq!(layout.runs[0].cells, 2, "the wide cell counts two columns");
        assert!(layout.runs[0].wide);
        assert_eq!(layout.runs[1].text, "x");
        assert_eq!(layout.runs[1].col, 2);
        assert!(!layout.runs[1].wide);
    }

    #[test]
    fn a_grapheme_cluster_is_spliced_from_the_rows_side_table() {
        let mut row = Row::new();
        row.extras.push("e\u{0301}".to_string());
        row.cells.push(PackedCell::new(
            0,
            StyleIdx::new(0),
            CellFlags::GRAPHEME_EXT,
        ));
        row.cells.push(PackedCell::from_char('!', StyleIdx::new(0)));
        let layout = layout_row(&row, 2, &table(&[]), &palette(), None);
        assert_eq!(layout.runs[0].text, "e\u{0301}!");
        assert_eq!(layout.runs[0].cells, 2);
    }

    #[test]
    fn a_selected_span_gets_its_own_background_and_style_key() {
        let row = text_row("abcdef", 0);
        let layout = layout_row(&row, 6, &table(&[]), &palette(), Some((2, 3)));
        assert_eq!(layout.backgrounds.len(), 1);
        assert_eq!(layout.backgrounds[0].col, 2);
        assert_eq!(layout.backgrounds[0].cells, 2);
        assert_eq!(layout.backgrounds[0].color, palette().selection_bg);
        // No selection foreground in the default palette, so the runs still
        // share a key and stay merged.
        assert_eq!(layout.runs.len(), 1);
    }

    #[test]
    fn a_selection_foreground_splits_the_run() {
        let mut palette = palette();
        palette.selection_fg = Some((0, 0, 0));
        let row = text_row("abcdef", 0);
        let layout = layout_row(&row, 6, &table(&[]), &palette, Some((2, 3)));
        assert_eq!(layout.runs.len(), 3);
        assert_eq!(layout.runs[1].text, "cd");
        assert_eq!(layout.runs[1].key.fg, (0, 0, 0));
    }

    #[test]
    fn an_underlined_run_of_spaces_is_kept_but_a_plain_one_is_dropped() {
        let underlined = Style {
            attrs: Attrs::UNDERLINE,
            ..Style::DEFAULT
        };
        let row = text_row("   ", 1);
        let layout = layout_row(&row, 3, &table(&[underlined]), &palette(), None);
        assert_eq!(layout.runs.len(), 1, "an underline under spaces is visible");

        let plain = text_row("   ", 0);
        let layout = layout_row(&plain, 3, &table(&[]), &palette(), None);
        assert!(layout.runs.is_empty());
    }

    #[test]
    fn a_hidden_cell_paints_its_glyph_in_the_background_colour() {
        let hidden = Style {
            attrs: Attrs::HIDDEN,
            bg: Color::Indexed(4),
            ..Style::DEFAULT
        };
        let row = text_row("secret", 1);
        let layout = layout_row(&row, 6, &table(&[hidden]), &palette(), None);
        let bg = palette().indexed(4);
        assert_eq!(layout.runs[0].key.fg, bg);
    }

    #[test]
    fn inverse_swaps_the_colours_before_the_quad_pass() {
        let inverse = Style {
            attrs: Attrs::INVERSE,
            ..Style::DEFAULT
        };
        let row = text_row("x", 1);
        let palette = palette();
        let layout = layout_row(&row, 1, &table(&[inverse]), &palette, None);
        assert_eq!(layout.runs[0].key.fg, palette.bg);
        assert_eq!(layout.backgrounds[0].color, palette.fg);
    }

    #[test]
    fn control_codepoints_never_reach_the_shaper() {
        let mut row = Row::new();
        row.cells
            .push(PackedCell::new(7, StyleIdx::new(0), CellFlags::empty()));
        row.cells.push(PackedCell::from_char('a', StyleIdx::new(0)));
        let layout = layout_row(&row, 2, &table(&[]), &palette(), None);
        assert_eq!(layout.runs[0].text, " a");
    }

    #[test]
    fn the_cache_counts_hits_and_misses() {
        let mut cache: RunCache<&str, u32> = RunCache::new(4);
        assert!(cache.get(&"a").is_none());
        cache.insert("a", 1);
        assert_eq!(cache.get(&"a"), Some(&1));
        assert_eq!(cache.counters(), (1, 1));
        assert!((cache.hit_rate() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn the_cache_evicts_the_least_recently_used_entries() {
        let mut cache: RunCache<u32, u32> = RunCache::new(8);
        for i in 0..8 {
            cache.insert(i, i);
        }
        // Keep 0..4 warm, then overflow.
        for i in 0..4 {
            assert_eq!(cache.get(&i), Some(&i));
        }
        cache.insert(100, 100);
        assert!(cache.len() <= 8);
        for i in 0..4 {
            assert!(cache.get(&i).is_some(), "recently used {i} was evicted");
        }
        assert!(cache.get(&100).is_some());
    }

    #[test]
    fn clearing_drops_the_lines_but_a_reset_only_drops_the_counters() {
        let mut cache: RunCache<u32, u32> = RunCache::new(4);
        cache.insert(1, 1);
        cache.get(&1);
        cache.reset_counters();
        assert_eq!(cache.counters(), (0, 0));
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn shrinking_the_capacity_evicts_immediately() {
        let mut cache: RunCache<u32, u32> = RunCache::new(16);
        for i in 0..16 {
            cache.insert(i, i);
        }
        cache.set_capacity(4);
        assert!(cache.len() <= 4, "{}", cache.len());
    }

    // ---- against a real Replica, fed the way the Data Plane feeds it ----

    fn snapshot_of(lines: &[&str], cols: u16) -> st_proto::Snapshot {
        let rows = u16::try_from(lines.len()).expect("test grid fits in u16");
        st_proto::Snapshot {
            surface_id: st_proto::SurfaceId(1),
            seq: st_proto::Seq::FIRST,
            cols,
            rows,
            styles: vec![st_proto::Style::DEFAULT],
            grid: lines.iter().map(|line| text_row(line, 0)).collect(),
            cursor: st_proto::Cursor::default(),
            modes: st_proto::Modes::empty(),
            title: "fake".to_string(),
            history_base: st_proto::AbsLine::ZERO,
            history_len: 0,
            view_state: st_proto::ViewState::default(),
            exited: None,
        }
    }

    fn replica_of(lines: &[&str], cols: u16) -> st_client_core::Replica {
        let mut replica = st_client_core::Replica::new(st_proto::SurfaceId(1));
        replica.apply_snapshot(&snapshot_of(lines, cols));
        replica
    }

    #[test]
    fn a_viewport_lays_out_every_row_a_snapshot_carried() {
        let replica = replica_of(&["hello world", "second line", ""], 20);
        let layouts = layout_viewport(
            &replica,
            20,
            3,
            replica.first_visible_line(),
            None,
            &palette(),
        );
        assert_eq!(layouts.len(), 3);
        assert_eq!(layouts[0].runs[0].text, "hello world");
        assert_eq!(layouts[1].runs[0].text, "second line");
        assert!(layouts[2].runs.is_empty(), "a blank row paints nothing");
    }

    #[test]
    fn a_viewport_above_the_cached_history_paints_blank_rather_than_stalling() {
        let replica = replica_of(&["only line"], 20);
        // Ask for lines the Replica has never seen: no panic, no rows.
        let layouts = layout_viewport(
            &replica,
            20,
            3,
            st_proto::AbsLine::new(9_000),
            None,
            &palette(),
        );
        assert_eq!(layouts.len(), 3);
        assert!(layouts.iter().all(|layout| layout.runs.is_empty()));
    }

    #[test]
    fn a_selection_over_a_real_replica_paints_a_selection_background() {
        use st_client_core::selection::{AbsPoint, SelectionMode};
        let replica = replica_of(&["hello world"], 20);
        let top = replica.first_visible_line();
        let mut selection = Selection::new(AbsPoint { line: top, col: 0 }, SelectionMode::Char);
        selection.extend_to(AbsPoint { line: top, col: 4 });
        let layouts = layout_viewport(&replica, 20, 1, top, Some(&selection), &palette());
        assert_eq!(layouts[0].backgrounds.len(), 1);
        assert_eq!(layouts[0].backgrounds[0].col, 0);
        assert_eq!(layouts[0].backgrounds[0].cells, 5);
        assert_eq!(layouts[0].backgrounds[0].color, palette().selection_bg);
    }

    #[test]
    fn narrowing_the_viewport_letterboxes_instead_of_reflowing() {
        // Q40: the element paints the intersection of its grid and the
        // Replica's until the next Delta catches up.
        let replica = replica_of(&["abcdefghij"], 10);
        let layouts = layout_viewport(
            &replica,
            4,
            1,
            replica.first_visible_line(),
            None,
            &palette(),
        );
        assert_eq!(layouts[0].runs[0].text, "abcd");
    }

    #[test]
    fn a_style_key_ignores_how_a_colour_was_reached() {
        let palette = palette();
        let direct = Style {
            fg: Color::Rgb(255, 0, 0),
            ..Style::DEFAULT
        };
        let indexed = Style {
            fg: Color::Indexed(1),
            ..Style::DEFAULT
        };
        let a = StyleKey::from_resolved(&palette.resolve_style(direct, false));
        let b = StyleKey::from_resolved(&palette.resolve_style(indexed, false));
        assert_eq!(a.fg == b.fg, palette.indexed(1) == (255, 0, 0));
        assert_eq!(a.bold, b.bold);
    }

    #[test]
    fn dim_and_bold_are_distinguishable_in_the_key() {
        let palette = palette();
        let bold = Style {
            attrs: Attrs::BOLD,
            ..Style::DEFAULT
        };
        let dim = Style {
            attrs: Attrs::DIM,
            ..Style::DEFAULT
        };
        let bold_key = StyleKey::from_resolved(&palette.resolve_style(bold, false));
        let dim_key = StyleKey::from_resolved(&palette.resolve_style(dim, false));
        assert!(bold_key.bold);
        assert!(!dim_key.bold);
        // DIM is folded into the colour, not carried as a flag.
        assert_ne!(bold_key.fg, dim_key.fg);
    }
}
