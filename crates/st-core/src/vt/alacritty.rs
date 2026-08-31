//! The `alacritty_terminal` implementation of [`VtEngine`] (ADR-0004, I6).
//!
//! **This is the only file in the workspace allowed to name
//! `alacritty_terminal`.** Everything it produces is `st-proto` data.
//!
//! Checked against `alacritty_terminal` 0.26.0 (which re-exports `vte` 0.15
//! with the `ansi` feature).
//!
//! # Deviations from `docs/plan/03-server.md` §4
//!
//! * **Eviction counting.** §4 proposed a `vte::ansi::Handler` shim wrapping
//!   `Term` to count lines pushed out of the ring (03-OQ5). That is not
//!   needed: this engine gives alacritty a scrollback limit *larger* than the
//!   configured one ([`HISTORY_SLACK`] extra lines) and does the trimming
//!   itself with `Grid::update_history`, so the number of evicted lines is
//!   known exactly rather than inferred. `advance` is chunked
//!   ([`ADVANCE_CHUNK`]) so the transient overshoot stays bounded.
//! * **Reflow.** `Term::resize` reflows the primary grid whenever the column
//!   count changes, which would renumber [`AbsLine`]s. Grilling Q40 forbids
//!   that, so [`AlacrittyEngine::resize`] swaps a scratch grid into the `Term`
//!   for the duration of the call — `Term::resize` then fixes up the tab
//!   stops, the scroll region, the damage state and the *inactive* grid while
//!   reflowing nothing but an empty placeholder — and resizes the real grid
//!   separately with `reflow = false`. While the alternate screen is active
//!   the two grids are swapped first so the protection lands on the grid that
//!   owns the history; the round trip resets the alternate screen's contents,
//!   which every full-screen program repaints on `SIGWINCH`.
//! * **Blink.** `alacritty_terminal` has no per-cell blink flag (SGR 5/6 are
//!   parsed and dropped), so [`st_proto::Attrs::BLINK`] is never set.
//! * **`Color::Named(BrightForeground | DimForeground | Cursor)`** collapse to
//!   [`st_proto::Color::Default`]: the wire format keeps colours symbolic so
//!   the Client's theme decides, and it has no slot for those three.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{ClipboardType, Config, Osc52, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as VtColor, CursorShape as VtCursorShape, Handler as _, NamedColor, Processor,
    Rgb as VtRgb,
};
use st_proto::{
    AbsLine, Attrs, CellFlags, Color, Cursor, CursorShape, Modes, PackedCell, Row, Style,
};

use crate::style_table::SurfaceStyleTable;
use crate::vt::{
    ColorReply, Damage, DirtySet, GridSnapshot, Rgb, SizeReply, TextAreaSize, VtEngine, VtEvent,
};

/// PTY bytes are handed to the parser in slices of at most this many bytes, so
/// that the self-managed history trim (see the module docs) runs often enough
/// to bound how far the ring can overshoot its configured size.
pub const ADVANCE_CHUNK: usize = 4 * 1024;

/// How many lines of headroom the engine gives `alacritty_terminal` above the
/// configured scrollback, so that alacritty itself never evicts silently.
pub const HISTORY_SLACK: usize = ADVANCE_CHUNK + 1;

/// Largest scrollback the engine will configure (`03-server.md` §4).
pub const MAX_SCROLLBACK_LINES: usize = 100_000;

/// How to build an [`AlacrittyEngine`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    /// Initial grid width.
    pub cols: u16,
    /// Initial grid height.
    pub rows: u16,
    /// Retained history lines, clamped to [`MAX_SCROLLBACK_LINES`].
    pub scrollback_lines: usize,
    /// Title used before OSC 0/2 sets one, and after `ResetTitle`.
    pub default_title: String,
    /// Advertise the Kitty keyboard protocol.
    pub kitty_keyboard: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            scrollback_lines: 10_000,
            default_title: String::new(),
            kitty_keyboard: true,
        }
    }
}

/// Grid dimensions, the shape `alacritty_terminal` asks for.
#[derive(Debug, Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Bridges alacritty's `EventListener` into a plain channel drained after each
/// `advance` (`03-server.md` §4).
#[derive(Debug, Clone)]
struct Listener(Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        // A closed receiver means the engine is being dropped; nothing to do.
        let _ = self.0.send(event);
    }
}

/// `alacritty_terminal`-backed [`VtEngine`].
pub struct AlacrittyEngine {
    term: Term<Listener>,
    processor: Processor,
    events: Receiver<Event>,
    /// Lines the ring is allowed to retain before this engine trims it.
    max_history: usize,
    /// Limit handed to alacritty, always `max_history + HISTORY_SLACK`.
    alac_history: usize,
    /// Absolute id of the oldest line still in the primary grid's history.
    history_base: u64,
    /// History size of the *primary* grid, kept across alt-screen excursions.
    primary_history_len: u64,
    title: String,
    default_title: String,
    force_full_damage: bool,
}

impl std::fmt::Debug for AlacrittyEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlacrittyEngine")
            .field("cols", &self.term.columns())
            .field("rows", &self.term.screen_lines())
            .field("history_base", &self.history_base)
            .field("history_len", &self.primary_history_len)
            .field("title", &self.title)
            .finish()
    }
}

impl AlacrittyEngine {
    /// Builds an engine with a blank grid of `config.cols` × `config.rows`.
    #[must_use]
    pub fn new(config: EngineConfig) -> Self {
        let cols = config.cols.max(1) as usize;
        let rows = config.rows.max(1) as usize;
        let max_history = config.scrollback_lines.min(MAX_SCROLLBACK_LINES);
        let alac_history = max_history + HISTORY_SLACK;

        let (tx, rx) = std::sync::mpsc::channel();
        let term_config = Config {
            scrolling_history: alac_history,
            kitty_keyboard: config.kitty_keyboard,
            // Grilling Q48: OSC 52 clipboard access is off in v1.
            osc52: Osc52::Disabled,
            ..Config::default()
        };
        let term = Term::new(term_config, &Size { cols, rows }, Listener(tx));

        Self {
            term,
            processor: Processor::new(),
            events: rx,
            max_history,
            alac_history,
            history_base: 0,
            primary_history_len: 0,
            title: config.default_title.clone(),
            default_title: config.default_title,
            force_full_damage: true,
        }
    }

    /// `true` while the alternate screen is active.
    #[must_use]
    pub fn is_alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// Retained history lines of the primary grid, even while the alternate
    /// screen is active.
    #[must_use]
    pub fn primary_history_len(&self) -> u64 {
        self.primary_history_len
    }

    /// Trims the ring back to the configured size and accounts for what was
    /// dropped, so [`AbsLine`]s stay stable across eviction.
    fn trim_history(&mut self) {
        if self.is_alt_screen() {
            return;
        }
        let history = self.term.grid().history_size();
        if history > self.max_history {
            let evicted = (history - self.max_history) as u64;
            // `update_history` drops the *oldest* lines and then pins the
            // limit; the second call restores the headroom.
            self.term.grid_mut().update_history(self.max_history);
            self.term.grid_mut().update_history(self.alac_history);
            self.history_base = self.history_base.saturating_add(evicted);
        }
        self.primary_history_len = self.term.grid().history_size() as u64;
    }

    /// The absolute id of the first visible row.
    fn first_visible_abs(&self) -> u64 {
        self.history_base + self.primary_history_len
    }

    /// Grid line index (viewport-relative, negative = history) of an absolute
    /// line id, or `None` when the id is outside the addressable range.
    fn grid_line_of(&self, abs: u64) -> Option<i32> {
        let first_visible = self.first_visible_abs();
        let rows = self.term.screen_lines() as u64;
        let lowest = self.history_base;
        if abs < lowest || abs >= first_visible + rows {
            return None;
        }
        Some((abs as i64 - first_visible as i64) as i32)
    }

    fn resize_active_without_reflow(&mut self, cols: usize, rows: usize) {
        let old_cols = self.term.columns();
        let old_rows = self.term.screen_lines();
        if cols == old_cols && rows == old_rows {
            return;
        }
        // Swap an empty grid of the *old* size in, so `Term::resize` sees the
        // real transition (it early-returns on unchanged dimensions) and does
        // all of its bookkeeping, but reflows only the placeholder.
        let placeholder = Grid::<Cell>::new(old_rows, old_cols, 0);
        let mut real = std::mem::replace(self.term.grid_mut(), placeholder);
        self.term.resize(Size { cols, rows });
        real.resize::<VtColor>(false, rows, cols);
        *self.term.grid_mut() = real;
    }

    fn packed_row(&self, line: i32, styles: &mut SurfaceStyleTable) -> Row {
        pack_row(&self.term.grid()[Line(line)], self.term.columns(), styles)
    }
}

impl VtEngine for AlacrittyEngine {
    fn advance(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(ADVANCE_CHUNK) {
            self.processor.advance(&mut self.term, chunk);
            self.trim_history();
        }
    }

    fn drain_events(&mut self) -> Vec<VtEvent> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Title(title) => {
                    self.title = title.clone();
                    out.push(VtEvent::Title(title));
                }
                Event::ResetTitle => {
                    self.title = self.default_title.clone();
                    out.push(VtEvent::ResetTitle);
                }
                Event::Bell => out.push(VtEvent::Bell),
                Event::PtyWrite(text) => out.push(VtEvent::PtyWrite(text.into_bytes())),
                Event::ClipboardStore(kind, text) => out.push(VtEvent::ClipboardStore {
                    kind: clipboard_byte(kind),
                    text,
                }),
                Event::ColorRequest(index, format) => out.push(VtEvent::ColorRequest {
                    index,
                    reply: ColorReply::new(Arc::new(move |rgb: Rgb| {
                        format(VtRgb {
                            r: rgb.r,
                            g: rgb.g,
                            b: rgb.b,
                        })
                    })),
                }),
                Event::TextAreaSizeRequest(format) => out.push(VtEvent::TextAreaSizeRequest {
                    reply: SizeReply::new(Arc::new(move |size: TextAreaSize| {
                        format(WindowSize {
                            num_lines: size.rows,
                            num_cols: size.cols,
                            cell_width: size.cell_width,
                            cell_height: size.cell_height,
                        })
                    })),
                }),
                // OSC 52 reads are disabled (Q48); the rest belong to
                // alacritty's own event loop, which we do not run.
                Event::ClipboardLoad(..)
                | Event::MouseCursorDirty
                | Event::CursorBlinkingChange
                | Event::Wakeup
                | Event::Exit
                | Event::ChildExit(_) => {}
            }
        }
        out
    }

    fn take_damage(&mut self) -> Damage {
        let rows = self.term.screen_lines();
        let mut set = DirtySet::new(rows);
        let mut full = std::mem::replace(&mut self.force_full_damage, false);
        match self.term.damage() {
            TermDamage::Full => full = true,
            TermDamage::Partial(lines) => {
                for bounds in lines {
                    set.set(bounds.line);
                }
            }
        }
        self.term.reset_damage();
        if full {
            Damage::Full
        } else {
            Damage::Rows(set)
        }
    }

    fn snapshot(&self, styles: &mut SurfaceStyleTable) -> GridSnapshot {
        let rows = self.term.screen_lines();
        let grid = (0..rows)
            .map(|line| self.packed_row(line as i32, styles))
            .collect();
        let (cursor, modes) = self.cursor_and_modes();
        GridSnapshot {
            cols: self.cols(),
            rows: self.rows(),
            grid,
            cursor,
            modes,
            title: self.title.clone(),
            history_base: self.history_base(),
            history_len: self.history_len(),
        }
    }

    fn row(&self, line: u16, styles: &mut SurfaceStyleTable) -> Row {
        let line = (line as usize).min(self.term.screen_lines().saturating_sub(1));
        self.packed_row(line as i32, styles)
    }

    fn cursor_and_modes(&self) -> (Cursor, Modes) {
        let mode = *self.term.mode();
        let mut point = self.term.grid().cursor.point;
        if self.term.grid()[point]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
            && point.column.0 > 0
        {
            point.column -= 1;
        }
        let style = self.term.cursor_style();
        let cursor = Cursor {
            row: point.line.0.clamp(0, self.term.screen_lines() as i32 - 1) as u16,
            col: point.column.0.min(self.term.columns().saturating_sub(1)) as u16,
            shape: match style.shape {
                VtCursorShape::Underline => CursorShape::Underline,
                VtCursorShape::Beam => CursorShape::Beam,
                _ => CursorShape::Block,
            },
            visible: mode.contains(TermMode::SHOW_CURSOR) && style.shape != VtCursorShape::Hidden,
            blink: style.blinking,
        };
        (cursor, map_modes(mode))
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(1) as usize;
        let rows = rows.max(1) as usize;
        if cols == self.term.columns() && rows == self.term.screen_lines() {
            return;
        }
        let was_alt = self.is_alt_screen();
        if was_alt {
            // Bring the history-owning grid back into `Term::grid` so the
            // reflow-free path protects it. alt -> primary clears nothing.
            self.term.swap_alt();
        }
        self.resize_active_without_reflow(cols, rows);
        if was_alt {
            // primary -> alt resets the alternate screen; programs repaint.
            self.term.swap_alt();
        }
        self.force_full_damage = true;
        self.trim_history();
    }

    fn cols(&self) -> u16 {
        self.term.columns() as u16
    }

    fn rows(&self) -> u16 {
        self.term.screen_lines() as u16
    }

    fn history_base(&self) -> AbsLine {
        if self.is_alt_screen() {
            // The alternate screen has no history; anchor it so that
            // `history_base + history_len` is still the first visible line.
            AbsLine::new(self.first_visible_abs())
        } else {
            AbsLine::new(self.history_base)
        }
    }

    fn history_len(&self) -> u64 {
        if self.is_alt_screen() {
            0
        } else {
            self.primary_history_len
        }
    }

    fn history_lines(&self, from: AbsLine, count: u32, styles: &mut SurfaceStyleTable) -> Vec<Row> {
        if count == 0 {
            return Vec::new();
        }
        let lowest = self.history_base().get();
        let start = from.get().max(lowest);
        let end_of_grid = self.first_visible_abs() + self.term.screen_lines() as u64;
        let end = end_of_grid.min(start.saturating_add(u64::from(count)));
        let mut out = Vec::new();
        let mut abs = start;
        while abs < end {
            match self.grid_line_of(abs) {
                Some(line) => out.push(self.packed_row(line, styles)),
                None => break,
            }
            abs += 1;
        }
        out
    }

    fn reset(&mut self) {
        // Everything that existed is retired, so ids are never reused.
        self.history_base = self
            .history_base
            .saturating_add(self.primary_history_len)
            .saturating_add(self.term.screen_lines() as u64);
        self.term.reset_state();
        self.primary_history_len = 0;
        self.title = self.default_title.clone();
        self.force_full_damage = true;
    }
}

fn clipboard_byte(kind: ClipboardType) -> u8 {
    match kind {
        ClipboardType::Clipboard => b'c',
        ClipboardType::Selection => b'p',
    }
}

/// Maps alacritty's mode bits onto the wire's [`Modes`] (`03-server.md` §4).
fn map_modes(mode: TermMode) -> Modes {
    let mut out = Modes::empty();
    out.set(Modes::ALT_SCREEN, mode.contains(TermMode::ALT_SCREEN));
    out.set(
        Modes::BRACKETED_PASTE,
        mode.contains(TermMode::BRACKETED_PASTE),
    );
    out.set(
        Modes::MOUSE_CLICK,
        mode.contains(TermMode::MOUSE_REPORT_CLICK),
    );
    out.set(Modes::MOUSE_DRAG, mode.contains(TermMode::MOUSE_DRAG));
    out.set(Modes::MOUSE_MOTION, mode.contains(TermMode::MOUSE_MOTION));
    out.set(Modes::MOUSE_SGR, mode.contains(TermMode::SGR_MOUSE));
    out.set(Modes::APP_CURSOR_KEYS, mode.contains(TermMode::APP_CURSOR));
    out.set(Modes::APP_KEYPAD, mode.contains(TermMode::APP_KEYPAD));
    out.set(Modes::FOCUS_EVENTS, mode.contains(TermMode::FOCUS_IN_OUT));
    out.set(Modes::LINE_WRAP, mode.contains(TermMode::LINE_WRAP));
    out.set(
        Modes::KITTY_KEYBOARD,
        mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL),
    );
    out
}

/// Maps an alacritty colour onto the wire's symbolic [`Color`].
fn map_color(color: VtColor) -> Color {
    match color {
        VtColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        VtColor::Indexed(index) => Color::Indexed(index),
        VtColor::Named(named) => map_named_color(named),
    }
}

fn map_named_color(named: NamedColor) -> Color {
    match named {
        NamedColor::Foreground
        | NamedColor::Background
        | NamedColor::Cursor
        | NamedColor::BrightForeground
        | NamedColor::DimForeground => Color::Default,
        NamedColor::DimBlack => Color::Indexed(0),
        NamedColor::DimRed => Color::Indexed(1),
        NamedColor::DimGreen => Color::Indexed(2),
        NamedColor::DimYellow => Color::Indexed(3),
        NamedColor::DimBlue => Color::Indexed(4),
        NamedColor::DimMagenta => Color::Indexed(5),
        NamedColor::DimCyan => Color::Indexed(6),
        NamedColor::DimWhite => Color::Indexed(7),
        // Black..BrightWhite are 0..=15 by definition.
        other => Color::Indexed(other as u8),
    }
}

/// Maps alacritty's per-cell flags onto the wire's [`Attrs`].
fn map_attrs(flags: Flags) -> Attrs {
    let mut attrs = Attrs::empty();
    attrs.set(Attrs::BOLD, flags.contains(Flags::BOLD));
    attrs.set(Attrs::DIM, flags.contains(Flags::DIM));
    attrs.set(Attrs::ITALIC, flags.contains(Flags::ITALIC));
    attrs.set(Attrs::INVERSE, flags.contains(Flags::INVERSE));
    attrs.set(Attrs::HIDDEN, flags.contains(Flags::HIDDEN));
    attrs.set(Attrs::STRIKETHROUGH, flags.contains(Flags::STRIKEOUT));
    if flags.contains(Flags::UNDERLINE) {
        attrs |= Attrs::UNDERLINE;
    } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
        attrs |= Attrs::UNDERLINE | Attrs::UL_DOUBLE;
    } else if flags.contains(Flags::UNDERCURL) {
        attrs |= Attrs::UNDERLINE | Attrs::UL_CURLY;
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        attrs |= Attrs::UNDERLINE | Attrs::UL_DOTTED;
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        attrs |= Attrs::UNDERLINE | Attrs::UL_DASHED;
    }
    attrs
}

/// The [`Style`] of one alacritty cell.
fn cell_style(cell: &Cell) -> Style {
    Style {
        fg: map_color(cell.fg),
        bg: map_color(cell.bg),
        underline_color: cell.underline_color().map_or(Color::Default, map_color),
        attrs: map_attrs(cell.flags),
    }
}

/// Packs one alacritty grid row into a wire [`Row`], trimming trailing blanks
/// and carrying the soft-wrap flag (grilling Q41).
fn pack_row(
    row: &alacritty_terminal::grid::Row<Cell>,
    cols: usize,
    styles: &mut SurfaceStyleTable,
) -> Row {
    let width = cols.min(row.len());
    let mut cells = Vec::with_capacity(width);
    let mut extras: Vec<String> = Vec::new();
    for column in 0..width {
        let cell = &row[Column(column)];
        let style_idx = styles.intern(cell_style(cell));
        let mut flags = CellFlags::empty();
        flags.set(CellFlags::WIDE, cell.flags.contains(Flags::WIDE_CHAR));
        flags.set(
            CellFlags::WIDE_SPACER,
            cell.flags.contains(Flags::WIDE_CHAR_SPACER),
        );
        flags.set(
            CellFlags::WIDE_LEADING_SPACER,
            cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER),
        );

        let zerowidth = cell.zerowidth().unwrap_or(&[]);
        let codepoint = if flags.contains(CellFlags::WIDE_SPACER) {
            0
        } else if zerowidth.is_empty() {
            cell.c as u32
        } else {
            let mut grapheme = String::with_capacity(1 + zerowidth.len());
            grapheme.push(cell.c);
            grapheme.extend(zerowidth.iter().copied());
            extras.push(grapheme);
            flags |= CellFlags::GRAPHEME_EXT;
            (extras.len() - 1) as u32
        };
        cells.push(PackedCell::new(codepoint, style_idx, flags));
    }

    let wrapped = width > 0 && row[Column(width - 1)].flags.contains(Flags::WRAPLINE);
    let mut out = Row {
        cells,
        extras,
        wrapped,
    };
    out.trim_trailing_blanks();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_proto::CursorShape;

    fn engine(cols: u16, rows: u16, scrollback: usize) -> AlacrittyEngine {
        AlacrittyEngine::new(EngineConfig {
            cols,
            rows,
            scrollback_lines: scrollback,
            default_title: "st".into(),
            kitty_keyboard: true,
        })
    }

    #[test]
    fn named_colours_map_to_symbolic_slots() {
        assert_eq!(map_named_color(NamedColor::Foreground), Color::Default);
        assert_eq!(map_named_color(NamedColor::Background), Color::Default);
        assert_eq!(map_named_color(NamedColor::Cursor), Color::Default);
        assert_eq!(map_named_color(NamedColor::Black), Color::Indexed(0));
        assert_eq!(map_named_color(NamedColor::White), Color::Indexed(7));
        assert_eq!(map_named_color(NamedColor::BrightBlack), Color::Indexed(8));
        assert_eq!(map_named_color(NamedColor::BrightWhite), Color::Indexed(15));
        assert_eq!(map_named_color(NamedColor::DimRed), Color::Indexed(1));
        assert_eq!(map_color(VtColor::Indexed(200)), Color::Indexed(200));
        assert_eq!(
            map_color(VtColor::Spec(VtRgb { r: 1, g: 2, b: 3 })),
            Color::Rgb(1, 2, 3)
        );
    }

    #[test]
    fn underline_kinds_are_distinguished() {
        assert_eq!(map_attrs(Flags::UNDERLINE).underline_kind(), 0);
        let curl = map_attrs(Flags::UNDERCURL);
        assert!(curl.contains(Attrs::UNDERLINE));
        assert_eq!(curl.underline_kind(), 2);
        assert_eq!(map_attrs(Flags::DOUBLE_UNDERLINE).underline_kind(), 1);
        assert_eq!(map_attrs(Flags::DOTTED_UNDERLINE).underline_kind(), 3);
        assert_eq!(map_attrs(Flags::DASHED_UNDERLINE).underline_kind(), 4);
        assert!(map_attrs(Flags::empty()).is_empty());
        // No blink flag exists upstream, so it is never set (see module docs).
        assert!(!map_attrs(Flags::all()).contains(Attrs::BLINK));
    }

    #[test]
    fn eviction_is_counted_exactly() {
        let mut e = engine(10, 3, 4);
        assert_eq!(e.history_base(), AbsLine::new(0));
        assert_eq!(e.history_len(), 0);

        // Six lines of content on a 3-row grid: 3 into the history, nothing
        // evicted yet.
        e.advance(b"1\r\n2\r\n3\r\n4\r\n5\r\n6");
        assert_eq!(e.history_len(), 3);
        assert_eq!(e.history_base(), AbsLine::new(0));

        // Two more push the ring past its 4-line limit.
        e.advance(b"\r\n7\r\n8");
        assert_eq!(e.history_len(), 4, "the ring is capped");
        assert_eq!(e.history_base(), AbsLine::new(1), "one line was evicted");

        e.advance(b"\r\n9\r\n10\r\n11");
        assert_eq!(e.history_len(), 4);
        assert_eq!(e.history_base(), AbsLine::new(4));

        // The id space is continuous: base + len + rows lines have existed.
        assert_eq!(e.history_base().get() + e.history_len() + 3, 11);
    }

    #[test]
    fn chunked_advance_matches_a_single_advance() {
        let script: Vec<u8> = (0..600)
            .flat_map(|i| format!("line {i}\r\n").into_bytes())
            .collect();
        let mut whole = engine(20, 5, 50);
        whole.advance(&script);

        let mut split = engine(20, 5, 50);
        for chunk in script.chunks(37) {
            split.advance(chunk);
        }

        assert_eq!(whole.history_base(), split.history_base());
        assert_eq!(whole.history_len(), split.history_len());
        let mut a = SurfaceStyleTable::new();
        let mut b = SurfaceStyleTable::new();
        assert_eq!(whole.snapshot(&mut a), split.snapshot(&mut b));
    }

    #[test]
    fn damage_is_row_granular_and_resets() {
        let mut e = engine(20, 4, 10);
        assert!(e.take_damage().is_full(), "a fresh grid is fully damaged");
        // Taking damage resets it; alacritty always re-reports the cursor cell
        // so a stale cursor block is repainted, hence row 0 and not nothing.
        match e.take_damage() {
            Damage::Rows(set) => assert_eq!(set.iter().collect::<Vec<_>>(), vec![0]),
            Damage::Full => panic!("damage was not reset"),
        }

        e.advance(b"\x1b[3;1Habc");
        match e.take_damage() {
            // Row 0 is where the cursor was, row 2 is where it went.
            Damage::Rows(set) => assert_eq!(set.iter().collect::<Vec<_>>(), vec![0, 2]),
            Damage::Full => panic!("a single-row write is not full damage"),
        }

        // A resize is always full damage.
        e.resize(10, 4);
        assert!(e.take_damage().is_full());
    }

    #[test]
    fn resize_does_not_reflow_the_history() {
        let mut e = engine(10, 2, 20);
        e.advance(b"abcdefghijklmnopqrst\r\nx");
        let base = e.history_base();
        let len = e.history_len();
        assert!(len > 0);

        e.resize(40, 2);
        assert_eq!(e.history_base(), base);
        assert_eq!(e.history_len(), len, "a widening resize merged nothing");
        assert_eq!(e.cols(), 40);
        assert_eq!(e.rows(), 2);
    }

    #[test]
    fn alt_screen_resize_keeps_the_primary_history() {
        let mut e = engine(20, 3, 40);
        e.advance(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        let base = e.history_base();
        let len = e.history_len();
        assert!(len > 0);

        e.advance(b"\x1b[?1049h");
        assert!(e.is_alt_screen());
        assert_eq!(e.history_len(), 0);
        assert_eq!(e.primary_history_len(), len);

        // Keep the row count: changing it legitimately moves lines between the
        // history and the viewport, which would mask what this test is about.
        e.resize(60, 3);
        assert!(e.is_alt_screen());
        assert_eq!(e.cols(), 60);
        assert_eq!(e.rows(), 3);

        e.advance(b"\x1b[?1049l");
        assert!(!e.is_alt_screen());
        assert_eq!(e.history_base(), base, "the history was not reflowed");
        assert_eq!(e.history_len(), len);
    }

    #[test]
    fn events_carry_the_title_and_the_bell() {
        let mut e = engine(10, 2, 4);
        e.advance(b"\x1b]0;hello\x07\x07");
        let events = e.drain_events();
        assert!(events
            .iter()
            .any(|ev| matches!(ev, VtEvent::Title(t) if t == "hello")));
        assert!(events.iter().any(|ev| matches!(ev, VtEvent::Bell)));
        assert_eq!(e.title(), "hello");
        assert!(e.drain_events().is_empty(), "events are drained once");
    }

    #[test]
    fn cursor_shape_and_visibility() {
        let mut e = engine(10, 2, 4);
        let (cursor, modes) = e.cursor_and_modes();
        assert_eq!(cursor.shape, CursorShape::Block);
        assert!(cursor.visible);
        assert!(modes.contains(Modes::LINE_WRAP));

        e.advance(b"\x1b[3 q");
        assert_eq!(e.cursor_and_modes().0.shape, CursorShape::Underline);
        e.advance(b"\x1b[?25l");
        assert!(!e.cursor_and_modes().0.visible);
    }

    #[test]
    fn history_lines_cover_history_and_the_visible_grid() {
        let mut e = engine(10, 2, 10);
        e.advance(b"a\r\nb\r\nc\r\nd");
        let mut styles = SurfaceStyleTable::new();
        let base = e.history_base();
        let rows = e.history_lines(base, 100, &mut styles);
        assert_eq!(rows.len(), (e.history_len() + u64::from(e.rows())) as usize);
        assert!(e
            .history_lines(AbsLine::new(9_999), 10, &mut styles)
            .is_empty());
        assert!(e.history_lines(base, 0, &mut styles).is_empty());
    }
}
