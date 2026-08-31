//! Grid → text rendering for `st probe`.
//!
//! One output line per grid row. The rules come straight from
//! `docs/plan/02-protocol.md`:
//!
//! * §4.4 — `cells.len() <= cols`; trailing [`PackedCell::BLANK`] cells are
//!   trimmed by the sender. This renderer trims again (a server is allowed to
//!   send untrimmed rows) so a row of spaces prints as an empty line.
//! * §5.1 — [`CellFlags::WIDE_SPACER`] is the second half of a two-column
//!   glyph and renders nothing; the leading [`CellFlags::WIDE`] cell already
//!   carries the character. [`CellFlags::WIDE_LEADING_SPACER`] is row-end
//!   filler and renders as a space.
//! * §5.2 — [`CellFlags::GRAPHEME_EXT`] means `codepoint` indexes the row's
//!   `extras`, which hold the full cluster.
//! * §5.3 — with `--color`, each style change emits one SGR sequence built
//!   from the [`Style`] the style table holds for that cell.

use std::fmt::Write as _;

use st_proto::{Attrs, CellFlags, Color, PackedCell, Row, Style, StyleTable};

use crate::replica::Replica;

/// Substituted for a `GRAPHEME_EXT` cell whose `extras` entry is missing, and
/// for a codepoint that is not a Unicode scalar. Both are server bugs; the
/// renderer must still produce a line.
pub const REPLACEMENT: char = '\u{FFFD}';

/// How to turn a grid into text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOptions {
    /// Emit ANSI SGR sequences from the style table instead of stripping
    /// styles.
    pub color: bool,
    /// Drop the trailing run of blank cells from every row (§4.4). On by
    /// default: it is what makes `st probe` output diffable.
    pub trim_trailing_blanks: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            color: false,
            trim_trailing_blanks: true,
        }
    }
}

impl RenderOptions {
    /// Plain text: no escapes at all.
    #[must_use]
    pub const fn plain() -> Self {
        Self {
            color: false,
            trim_trailing_blanks: true,
        }
    }

    /// The same, but emitting SGR from the style table.
    #[must_use]
    pub const fn colored() -> Self {
        Self {
            color: true,
            trim_trailing_blanks: true,
        }
    }
}

/// Renders the whole visible grid: one line per row, each terminated by `\n`.
#[must_use]
pub fn render_grid(replica: &Replica, opts: RenderOptions) -> String {
    let mut out = String::with_capacity(replica.grid.len() * (replica.cols as usize + 1));
    for row in &replica.grid {
        out.push_str(&render_row(row, replica.cols, &replica.styles, opts));
        out.push('\n');
    }
    out
}

/// Renders a single row, without a trailing newline.
#[must_use]
pub fn render_row(row: &Row, cols: u16, styles: &StyleTable, opts: RenderOptions) -> String {
    let width = (cols as usize).min(row.cells.len());
    let visible = &row.cells[..width];

    let end = if opts.trim_trailing_blanks {
        visible
            .iter()
            .rposition(|c| !c.is_blank())
            .map_or(0, |i| i + 1)
    } else {
        visible.len()
    };

    let mut out = String::with_capacity(end + 8);
    let mut current = Style::DEFAULT;

    for cell in &visible[..end] {
        if cell.flags.contains(CellFlags::WIDE_SPACER) {
            // Second column of a wide glyph: the leading cell printed it.
            continue;
        }
        if opts.color {
            let style = styles.get_or_default(cell.style_idx);
            if style != current {
                out.push_str(&sgr(style));
                current = style;
            }
        }
        push_text(&mut out, row, *cell);
    }

    if opts.color && current != Style::DEFAULT {
        out.push_str("\x1b[0m");
    }
    out
}

/// Appends the text a cell displays.
fn push_text(out: &mut String, row: &Row, cell: PackedCell) {
    if cell.flags.contains(CellFlags::GRAPHEME_EXT) {
        match row.grapheme(cell) {
            Some(cluster) => out.push_str(cluster),
            None => out.push(REPLACEMENT),
        }
        return;
    }
    if cell.codepoint == 0 {
        // WIDE_LEADING_SPACER, and any cell the server left empty.
        out.push(' ');
        return;
    }
    out.push(char::from_u32(cell.codepoint).unwrap_or(REPLACEMENT));
}

/// Builds the SGR sequence that selects `style`, starting from a reset.
///
/// Always leading with `0` makes each sequence absolute, so a renderer never
/// has to compute the difference between two styles.
#[must_use]
pub fn sgr(style: Style) -> String {
    let mut params = String::from("0");
    let attrs = style.attrs;

    for (flag, code) in [(Attrs::BOLD, "1"), (Attrs::DIM, "2"), (Attrs::ITALIC, "3")] {
        if attrs.contains(flag) {
            params.push(';');
            params.push_str(code);
        }
    }
    if attrs.contains(Attrs::UNDERLINE) {
        params.push(';');
        params.push_str(match attrs.underline_kind() {
            1 => "21",
            2 => "4:3",
            3 => "4:4",
            4 => "4:5",
            _ => "4",
        });
    }
    for (flag, code) in [
        (Attrs::BLINK, "5"),
        (Attrs::INVERSE, "7"),
        (Attrs::HIDDEN, "8"),
        (Attrs::STRIKETHROUGH, "9"),
    ] {
        if attrs.contains(flag) {
            params.push(';');
            params.push_str(code);
        }
    }

    push_color(&mut params, style.fg, ColorRole::Fg);
    push_color(&mut params, style.bg, ColorRole::Bg);
    push_color(&mut params, style.underline_color, ColorRole::Underline);

    format!("\x1b[{params}m")
}

#[derive(Clone, Copy)]
enum ColorRole {
    Fg,
    Bg,
    Underline,
}

impl ColorRole {
    /// The `38`/`48`/`58` extended-colour introducer.
    const fn extended(self) -> u8 {
        match self {
            ColorRole::Fg => 38,
            ColorRole::Bg => 48,
            ColorRole::Underline => 58,
        }
    }

    /// The base of the 8-colour range, or `None` for underline colour, which
    /// has no short form.
    const fn short_base(self) -> Option<(u8, u8)> {
        match self {
            ColorRole::Fg => Some((30, 90)),
            ColorRole::Bg => Some((40, 100)),
            ColorRole::Underline => None,
        }
    }
}

fn push_color(params: &mut String, color: Color, role: ColorRole) {
    match color {
        // A leading `0` already reset every colour to its default.
        Color::Default => {}
        Color::Indexed(idx) => match (role.short_base(), idx) {
            (Some((base, _)), 0..=7) => {
                let _ = write!(params, ";{}", base + idx);
            }
            (Some((_, bright)), 8..=15) => {
                let _ = write!(params, ";{}", bright + (idx - 8));
            }
            _ => {
                let _ = write!(params, ";{};5;{idx}", role.extended());
            }
        },
        Color::Rgb(r, g, b) => {
            let _ = write!(params, ";{};2;{r};{g};{b}", role.extended());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_proto::{StyleIdx, SurfaceId};

    fn table(extra: &[Style]) -> StyleTable {
        let mut styles = vec![Style::DEFAULT];
        styles.extend_from_slice(extra);
        StyleTable::from_wire(&styles).unwrap()
    }

    fn ascii(text: &str, idx: u16) -> Row {
        let mut row = Row::new();
        row.cells = text
            .chars()
            .map(|c| PackedCell::from_char(c, StyleIdx::new(idx)))
            .collect();
        row
    }

    #[test]
    fn plain_text_of_an_ascii_row() {
        let row = ascii("hello", 0);
        assert_eq!(
            render_row(&row, 80, &table(&[]), RenderOptions::plain()),
            "hello"
        );
    }

    #[test]
    fn trailing_blanks_are_trimmed_but_interior_spaces_are_not() {
        let mut row = ascii("a b   ", 0);
        row.cells.resize(20, PackedCell::BLANK);
        assert_eq!(
            render_row(&row, 20, &table(&[]), RenderOptions::plain()),
            "a b"
        );
        let untrimmed = render_row(
            &row,
            20,
            &table(&[]),
            RenderOptions {
                color: false,
                trim_trailing_blanks: false,
            },
        );
        assert_eq!(untrimmed, format!("a b{}", " ".repeat(17)));
    }

    #[test]
    fn an_all_blank_row_renders_as_an_empty_line() {
        let mut row = Row::new();
        row.cells = vec![PackedCell::BLANK; 10];
        assert_eq!(
            render_row(&row, 10, &table(&[]), RenderOptions::plain()),
            ""
        );
    }

    #[test]
    fn a_space_with_a_background_is_not_a_blank() {
        let styles = table(&[Style {
            bg: Color::Indexed(1),
            ..Style::DEFAULT
        }]);
        let mut row = Row::new();
        row.cells = vec![
            PackedCell::from_char('x', StyleIdx::ZERO),
            PackedCell::from_char(' ', StyleIdx::new(1)),
        ];
        assert_eq!(render_row(&row, 10, &styles, RenderOptions::plain()), "x ");
    }

    #[test]
    fn cells_past_cols_are_ignored() {
        let row = ascii("abcdefgh", 0);
        assert_eq!(
            render_row(&row, 3, &table(&[]), RenderOptions::plain()),
            "abc"
        );
    }

    #[test]
    fn a_wide_glyph_prints_once_and_its_spacer_prints_nothing() {
        let mut row = Row::new();
        row.cells = vec![
            PackedCell::new('世' as u32, StyleIdx::ZERO, CellFlags::WIDE),
            PackedCell::new(0, StyleIdx::ZERO, CellFlags::WIDE_SPACER),
            PackedCell::new('界' as u32, StyleIdx::ZERO, CellFlags::WIDE),
            PackedCell::new(0, StyleIdx::ZERO, CellFlags::WIDE_SPACER),
            PackedCell::from_char('!', StyleIdx::ZERO),
        ];
        assert_eq!(
            render_row(&row, 80, &table(&[]), RenderOptions::plain()),
            "世界!"
        );
    }

    #[test]
    fn a_wide_leading_spacer_renders_as_one_space() {
        let mut row = Row::new();
        row.cells = vec![
            PackedCell::from_char('a', StyleIdx::ZERO),
            PackedCell::new(0, StyleIdx::ZERO, CellFlags::WIDE_LEADING_SPACER),
        ];
        // The filler is not `BLANK` (its flags differ), so trimming keeps it.
        assert_eq!(
            render_row(&row, 80, &table(&[]), RenderOptions::plain()),
            "a "
        );
    }

    #[test]
    fn grapheme_clusters_come_from_extras() {
        let mut row = Row::new();
        row.extras = vec!["e\u{301}".into(), "👩‍👩‍👧".into()];
        row.cells = vec![
            PackedCell::new(0, StyleIdx::ZERO, CellFlags::GRAPHEME_EXT),
            PackedCell::new(1, StyleIdx::ZERO, CellFlags::GRAPHEME_EXT | CellFlags::WIDE),
            PackedCell::new(0, StyleIdx::ZERO, CellFlags::WIDE_SPACER),
            PackedCell::new(9, StyleIdx::ZERO, CellFlags::GRAPHEME_EXT),
        ];
        assert_eq!(
            render_row(&row, 80, &table(&[]), RenderOptions::plain()),
            format!("e\u{301}👩‍👩‍👧{REPLACEMENT}")
        );
    }

    #[test]
    fn sgr_for_the_default_style_is_a_bare_reset() {
        assert_eq!(sgr(Style::DEFAULT), "\x1b[0m");
    }

    #[test]
    fn sgr_covers_attributes_in_a_fixed_order() {
        let style = Style {
            attrs: Attrs::BOLD
                | Attrs::DIM
                | Attrs::ITALIC
                | Attrs::UNDERLINE
                | Attrs::BLINK
                | Attrs::INVERSE
                | Attrs::HIDDEN
                | Attrs::STRIKETHROUGH,
            ..Style::DEFAULT
        };
        assert_eq!(sgr(style), "\x1b[0;1;2;3;4;5;7;8;9m");
    }

    #[test]
    fn sgr_encodes_every_underline_kind() {
        let with = |kind: Attrs| {
            sgr(Style {
                attrs: Attrs::UNDERLINE | kind,
                ..Style::DEFAULT
            })
        };
        assert_eq!(with(Attrs::empty()), "\x1b[0;4m");
        assert_eq!(with(Attrs::UL_DOUBLE), "\x1b[0;21m");
        assert_eq!(with(Attrs::UL_CURLY), "\x1b[0;4:3m");
        assert_eq!(with(Attrs::UL_DOTTED), "\x1b[0;4:4m");
        assert_eq!(with(Attrs::UL_DASHED), "\x1b[0;4:5m");
    }

    #[test]
    fn sgr_encodes_every_colour_form() {
        let colored = |fg, bg, ul| {
            sgr(Style {
                fg,
                bg,
                underline_color: ul,
                attrs: Attrs::empty(),
            })
        };
        assert_eq!(
            colored(Color::Indexed(1), Color::Indexed(2), Color::Default),
            "\x1b[0;31;42m"
        );
        assert_eq!(
            colored(Color::Indexed(9), Color::Indexed(15), Color::Default),
            "\x1b[0;91;107m"
        );
        assert_eq!(
            colored(Color::Indexed(200), Color::Indexed(16), Color::Default),
            "\x1b[0;38;5;200;48;5;16m"
        );
        assert_eq!(
            colored(Color::Rgb(1, 2, 3), Color::Rgb(4, 5, 6), Color::Indexed(7)),
            "\x1b[0;38;2;1;2;3;48;2;4;5;6;58;5;7m"
        );
        assert_eq!(
            colored(Color::Default, Color::Default, Color::Rgb(9, 8, 7)),
            "\x1b[0;58;2;9;8;7m"
        );
    }

    #[test]
    fn colour_mode_emits_one_sgr_per_run_and_resets_at_end_of_line() {
        let styles = table(&[
            Style {
                fg: Color::Indexed(2),
                attrs: Attrs::BOLD,
                ..Style::DEFAULT
            },
            Style {
                fg: Color::Rgb(10, 20, 30),
                ..Style::DEFAULT
            },
        ]);
        let mut row = Row::new();
        row.cells = [('a', 0u16), ('b', 1), ('c', 1), ('d', 2), ('e', 0)]
            .into_iter()
            .map(|(c, i)| PackedCell::from_char(c, StyleIdx::new(i)))
            .collect();

        assert_eq!(
            render_row(&row, 80, &styles, RenderOptions::colored()),
            "a\x1b[0;1;32mbc\x1b[0;38;2;10;20;30md\x1b[0me"
        );
    }

    #[test]
    fn colour_mode_leaves_a_wholly_default_row_untouched() {
        let row = ascii("plain", 0);
        assert_eq!(
            render_row(&row, 80, &table(&[]), RenderOptions::colored()),
            "plain"
        );
    }

    #[test]
    fn colour_mode_closes_a_style_that_runs_to_the_end_of_the_line() {
        let styles = table(&[Style {
            attrs: Attrs::ITALIC,
            ..Style::DEFAULT
        }]);
        let row = ascii("hi", 1);
        assert_eq!(
            render_row(&row, 80, &styles, RenderOptions::colored()),
            "\x1b[0;3mhi\x1b[0m"
        );
    }

    #[test]
    fn a_whole_grid_is_newline_terminated_per_row() {
        let snap = st_proto::Snapshot {
            surface_id: SurfaceId(1),
            seq: st_proto::Seq(1),
            cols: 8,
            rows: 3,
            styles: vec![Style::DEFAULT],
            grid: vec![ascii("top", 0), Row::new(), ascii("bottom", 0)],
            cursor: st_proto::Cursor::default(),
            modes: st_proto::Modes::empty(),
            title: String::new(),
            history_base: st_proto::AbsLine(0),
            history_len: 0,
            view_state: st_proto::ViewState::default(),
            exited: None,
        };
        let replica = Replica::from_snapshot(&snap);
        assert_eq!(
            render_grid(&replica, RenderOptions::plain()),
            "top\n\nbottom\n"
        );
    }
}
