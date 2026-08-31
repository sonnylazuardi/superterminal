//! Colour resolution — `docs/plan/04-client-native.md` §10 and §6 step 8.
//!
//! A [`st_proto::Color`] on the wire is `Default`, `Indexed(0..=255)` or
//! `Rgb`; resolving it against the theme happens at *paint* time, not when a
//! Delta lands, so changing the theme never touches a Replica.
//!
//! Everything here produces plain [`Rgb`] triples — this crate has no GPUI
//! dependency (invariant I9), so the GPUI layer converts to `gpui::Rgba` at
//! the boundary.
//!
//! # The 256-colour table
//!
//! * `0..16` — the theme's ANSI colours.
//! * `16..232` — a 6×6×6 cube: `i - 16 = 36·r + 6·g + b`, each component
//!   mapping `0 → 0` and `c → 55 + 40·c`.
//! * `232..256` — a 24-step grey ramp, `8 + 10·(i - 232)`.
//!
//! ```
//! use st_client_core::palette::Palette;
//!
//! let palette = Palette::default();
//! assert_eq!(palette.indexed(16), (0, 0, 0));
//! assert_eq!(palette.indexed(231), (255, 255, 255));
//! assert_eq!(palette.indexed(244), (128, 128, 128));
//! ```

use st_proto::{Attrs, Color, Style};

/// An 8-bit-per-channel colour. Deliberately a plain tuple: no UI-toolkit
/// types cross this crate's boundary.
pub type Rgb = (u8, u8, u8);

/// The factor a dim (SGR 2) foreground is multiplied by (§6 step 8).
pub const DIM_FACTOR: f32 = 0.7;

/// The 16 ANSI colours of the default dark theme, xterm-ish
/// (`04-client-native.md` §10).
pub const DEFAULT_ANSI: [Rgb; 16] = [
    (0x1e, 0x1e, 0x1e), // 0 black
    (0xcd, 0x31, 0x31), // 1 red
    (0x0d, 0xbc, 0x79), // 2 green
    (0xe5, 0xe5, 0x10), // 3 yellow
    (0x24, 0x72, 0xc8), // 4 blue
    (0xbc, 0x3f, 0xbc), // 5 magenta
    (0x11, 0xa8, 0xcd), // 6 cyan
    (0xd4, 0xd4, 0xd4), // 7 white
    (0x66, 0x66, 0x66), // 8 bright black
    (0xf1, 0x4c, 0x4c), // 9 bright red
    (0x23, 0xd1, 0x8b), // 10 bright green
    (0xf5, 0xf5, 0x43), // 11 bright yellow
    (0x3b, 0x8e, 0xea), // 12 bright blue
    (0xd6, 0x70, 0xd6), // 13 bright magenta
    (0x29, 0xb8, 0xdb), // 14 bright cyan
    (0xff, 0xff, 0xff), // 15 bright white
];

/// A resolved theme.
///
/// Build one from `config.toml`'s `[theme]` (the app passes it down as the
/// `theme` prop); [`Palette::default`] is the neutral dark theme of §10.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    /// The 16 ANSI colours.
    ansi: [Rgb; 16],
    /// Default foreground.
    pub fg: Rgb,
    /// Default background.
    pub bg: Rgb,
    /// Cursor body colour.
    pub cursor: Rgb,
    /// Colour of the glyph under a block cursor.
    pub cursor_text: Rgb,
    /// Selection background.
    pub selection_bg: Rgb,
    /// Selection foreground; `None` keeps each cell's own foreground.
    pub selection_fg: Option<Rgb>,
    /// Bold text with an indexed foreground in `0..8` uses `8..16` instead
    /// (grilling Q48; default `false`).
    pub bold_is_bright: bool,
    /// The full 256-entry table, recomputed whenever [`ansi`](Palette::ansi)
    /// changes.
    table: [Rgb; 256],
}

impl Default for Palette {
    fn default() -> Self {
        Self::new(DEFAULT_ANSI, (0xd4, 0xd4, 0xd4), (0x1e, 0x1e, 0x1e))
    }
}

impl Palette {
    /// A palette from the 16 ANSI colours plus default fg/bg; the cursor and
    /// selection colours are derived and can be overwritten afterwards.
    #[must_use]
    pub fn new(ansi: [Rgb; 16], fg: Rgb, bg: Rgb) -> Self {
        let mut palette = Self {
            ansi,
            fg,
            bg,
            cursor: fg,
            cursor_text: bg,
            selection_bg: (0x26, 0x4f, 0x78),
            selection_fg: None,
            bold_is_bright: false,
            table: [(0, 0, 0); 256],
        };
        palette.rebuild();
        palette
    }

    /// The 16 ANSI colours.
    #[inline]
    #[must_use]
    pub const fn ansi(&self) -> &[Rgb; 16] {
        &self.ansi
    }

    /// Replaces the ANSI colours and recomputes the 256-entry table.
    pub fn set_ansi(&mut self, ansi: [Rgb; 16]) {
        self.ansi = ansi;
        self.rebuild();
    }

    /// Recomputes the 256-entry lookup table from [`ansi`](Palette::ansi).
    ///
    /// Called by every constructor and setter; call it yourself only after
    /// mutating the palette through a path that does not.
    pub fn rebuild(&mut self) {
        for (i, slot) in self.table.iter_mut().enumerate() {
            *slot = match i {
                0..=15 => self.ansi[i],
                16..=231 => cube_rgb(i as u8),
                _ => grey_rgb(i as u8),
            };
        }
    }

    /// The RGB of palette entry `index`.
    #[inline]
    #[must_use]
    pub const fn indexed(&self, index: u8) -> Rgb {
        self.table[index as usize]
    }

    /// The whole 256-entry table, for a renderer that wants to upload it once.
    #[inline]
    #[must_use]
    pub const fn table(&self) -> &[Rgb; 256] {
        &self.table
    }

    /// Resolves a foreground colour: `Default` becomes [`fg`](Palette::fg).
    #[must_use]
    pub fn resolve_fg(&self, color: Color) -> Rgb {
        match color {
            Color::Default => self.fg,
            Color::Indexed(i) => self.indexed(i),
            Color::Rgb(r, g, b) => (r, g, b),
        }
    }

    /// Resolves a background colour: `Default` becomes [`bg`](Palette::bg).
    #[must_use]
    pub fn resolve_bg(&self, color: Color) -> Rgb {
        match color {
            Color::Default => self.bg,
            Color::Indexed(i) => self.indexed(i),
            Color::Rgb(r, g, b) => (r, g, b),
        }
    }

    /// Resolves a whole [`Style`] into paintable colours (§6 step 8).
    ///
    /// The order matters and matches xterm:
    ///
    /// 1. bold + indexed `0..8` → `8..16`, when
    ///    [`bold_is_bright`](Palette::bold_is_bright);
    /// 2. `INVERSE` swaps foreground and background;
    /// 3. selection paints its own background (and foreground, if the theme
    ///    sets one) *after* the swap;
    /// 4. `DIM` scales the foreground by [`DIM_FACTOR`];
    /// 5. `HIDDEN` makes the foreground equal the background.
    #[must_use]
    pub fn resolve_style(&self, style: Style, selected: bool) -> ResolvedStyle {
        let attrs = style.attrs;

        let fg_color = if self.bold_is_bright && attrs.contains(Attrs::BOLD) {
            brighten(style.fg)
        } else {
            style.fg
        };

        let mut fg = self.resolve_fg(fg_color);
        let mut bg = self.resolve_bg(style.bg);
        // A cell with no explicit background shows the element's own
        // background; the renderer skips the quad for those.
        let mut bg_is_default = style.bg == Color::Default;

        if attrs.contains(Attrs::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
            bg_is_default = false;
        }

        if selected {
            bg = self.selection_bg;
            bg_is_default = false;
            if let Some(selection_fg) = self.selection_fg {
                fg = selection_fg;
            }
        }

        if attrs.contains(Attrs::DIM) {
            fg = scale(fg, DIM_FACTOR);
        }
        if attrs.contains(Attrs::HIDDEN) {
            fg = bg;
        }

        let underline = match style.underline_color {
            Color::Default => fg,
            other => self.resolve_fg(other),
        };

        ResolvedStyle {
            fg,
            bg,
            underline,
            bg_is_default,
            attrs,
        }
    }

    /// The colours a block cursor paints with.
    #[must_use]
    pub const fn cursor_colors(&self) -> (Rgb, Rgb) {
        (self.cursor, self.cursor_text)
    }
}

/// A [`Style`] with every colour resolved, ready to paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResolvedStyle {
    /// Glyph colour.
    pub fg: Rgb,
    /// Cell background colour.
    pub bg: Rgb,
    /// Underline colour (SGR 58), defaulting to [`fg`](ResolvedStyle::fg).
    pub underline: Rgb,
    /// `true` when the cell's background is the theme default *and* nothing
    /// overrode it, so the renderer can skip the quad and let a blurred window
    /// background show through (grilling Q28).
    pub bg_is_default: bool,
    /// The attributes that drove the resolution, for the glyph-run key.
    pub attrs: Attrs,
}

/// Maps indexed `0..8` onto `8..16`; everything else is unchanged.
fn brighten(color: Color) -> Color {
    match color {
        Color::Indexed(i) if i < 8 => Color::Indexed(i + 8),
        other => other,
    }
}

/// Multiplies each channel by `factor`, rounding to nearest.
fn scale(color: Rgb, factor: f32) -> Rgb {
    let c = |v: u8| (f32::from(v) * factor).round().clamp(0.0, 255.0) as u8;
    (c(color.0), c(color.1), c(color.2))
}

/// One component of the 6×6×6 cube: `0 → 0`, `c → 55 + 40·c`.
#[inline]
#[must_use]
pub const fn cube_component(c: u8) -> u8 {
    if c == 0 {
        0
    } else {
        55 + 40 * c
    }
}

/// The RGB of a cube index, `16..=231`. Indices outside that range are clamped
/// into it.
#[must_use]
pub const fn cube_rgb(index: u8) -> Rgb {
    let i = if index < 16 {
        0
    } else if index > 231 {
        215
    } else {
        index - 16
    };
    (
        cube_component(i / 36),
        cube_component((i / 6) % 6),
        cube_component(i % 6),
    )
}

/// The RGB of a grey-ramp index, `232..=255`: `8 + 10·(index - 232)`.
/// Indices below 232 are clamped to the first step.
#[must_use]
pub const fn grey_rgb(index: u8) -> Rgb {
    let step = index.saturating_sub(232);
    let level = 8 + 10 * step;
    (level, level, level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_spot_checks() {
        let p = Palette::default();
        // The corners of the cube.
        assert_eq!(p.indexed(16), (0, 0, 0));
        assert_eq!(p.indexed(231), (255, 255, 255));
        // 21 = 16 + 5 → blue at full component.
        assert_eq!(p.indexed(21), (0, 0, 255));
        // 46 = 16 + 30 → green at full component.
        assert_eq!(p.indexed(46), (0, 255, 0));
        // 196 = 16 + 180 → red at full component.
        assert_eq!(p.indexed(196), (255, 0, 0));
        // 100 = 16 + 84 → 84 = 36·2 + 6·2 + 0.
        assert_eq!(p.indexed(100), (135, 135, 0));
        // Component mapping.
        assert_eq!(cube_component(0), 0);
        assert_eq!(cube_component(1), 95);
        assert_eq!(cube_component(2), 135);
        assert_eq!(cube_component(3), 175);
        assert_eq!(cube_component(4), 215);
        assert_eq!(cube_component(5), 255);
    }

    #[test]
    fn grey_ramp_spot_checks() {
        let p = Palette::default();
        assert_eq!(p.indexed(232), (8, 8, 8));
        assert_eq!(p.indexed(244), (128, 128, 128));
        assert_eq!(p.indexed(255), (238, 238, 238));
        assert_eq!(grey_rgb(232), (8, 8, 8));
        assert_eq!(grey_rgb(0), (8, 8, 8));
    }

    #[test]
    fn the_first_sixteen_entries_are_the_theme() {
        let mut ansi = DEFAULT_ANSI;
        ansi[1] = (1, 2, 3);
        let mut p = Palette::default();
        p.set_ansi(ansi);
        assert_eq!(p.indexed(1), (1, 2, 3));
        assert_eq!(p.indexed(0), DEFAULT_ANSI[0]);
        assert_eq!(p.ansi()[1], (1, 2, 3));
        // Only the first 16 entries move.
        assert_eq!(p.indexed(16), (0, 0, 0));
        assert_eq!(p.table().len(), 256);
    }

    #[test]
    fn the_whole_table_is_covered_and_monotonic_where_it_should_be() {
        let p = Palette::default();
        for i in 16u8..=231 {
            let rgb = cube_rgb(i);
            assert_eq!(p.indexed(i), rgb);
        }
        // The grey ramp climbs by exactly 10 per step.
        for i in 233u8..=255 {
            assert_eq!(
                u16::from(p.indexed(i).0),
                u16::from(p.indexed(i - 1).0) + 10
            );
        }
        // Out-of-range cube indices clamp instead of wrapping.
        assert_eq!(cube_rgb(0), (0, 0, 0));
        assert_eq!(cube_rgb(255), (255, 255, 255));
    }

    #[test]
    fn truecolor_passes_straight_through() {
        let p = Palette::default();
        assert_eq!(p.resolve_fg(Color::Rgb(9, 8, 7)), (9, 8, 7));
        assert_eq!(p.resolve_bg(Color::Rgb(9, 8, 7)), (9, 8, 7));
        let style = Style {
            fg: Color::Rgb(1, 2, 3),
            bg: Color::Rgb(4, 5, 6),
            ..Style::DEFAULT
        };
        let r = p.resolve_style(style, false);
        assert_eq!(r.fg, (1, 2, 3));
        assert_eq!(r.bg, (4, 5, 6));
        assert!(!r.bg_is_default);
    }

    #[test]
    fn default_colours_come_from_the_theme() {
        let p = Palette::default();
        let r = p.resolve_style(Style::DEFAULT, false);
        assert_eq!(r.fg, p.fg);
        assert_eq!(r.bg, p.bg);
        assert!(r.bg_is_default);
        assert_eq!(r.underline, p.fg);
        assert_eq!(p.cursor_colors(), (p.fg, p.bg));
    }

    #[test]
    fn bold_is_bright_is_off_by_default() {
        let mut p = Palette::default();
        let bold_red = Style {
            fg: Color::Indexed(1),
            attrs: Attrs::BOLD,
            ..Style::DEFAULT
        };
        assert_eq!(p.resolve_style(bold_red, false).fg, DEFAULT_ANSI[1]);

        p.bold_is_bright = true;
        assert_eq!(p.resolve_style(bold_red, false).fg, DEFAULT_ANSI[9]);

        // Only indices 0..8 brighten; 8..16, cube entries and truecolor do not.
        let bold_bright = Style {
            fg: Color::Indexed(9),
            attrs: Attrs::BOLD,
            ..Style::DEFAULT
        };
        assert_eq!(p.resolve_style(bold_bright, false).fg, DEFAULT_ANSI[9]);
        let bold_cube = Style {
            fg: Color::Indexed(200),
            attrs: Attrs::BOLD,
            ..Style::DEFAULT
        };
        assert_eq!(p.resolve_style(bold_cube, false).fg, p.indexed(200));
        let bold_default = Style {
            attrs: Attrs::BOLD,
            ..Style::DEFAULT
        };
        assert_eq!(p.resolve_style(bold_default, false).fg, p.fg);
    }

    #[test]
    fn dim_scales_the_foreground_by_seven_tenths() {
        let p = Palette::default();
        let dim = Style {
            fg: Color::Rgb(100, 200, 255),
            attrs: Attrs::DIM,
            ..Style::DEFAULT
        };
        assert_eq!(p.resolve_style(dim, false).fg, (70, 140, 179));
        // The background is untouched.
        assert_eq!(p.resolve_style(dim, false).bg, p.bg);
    }

    #[test]
    fn inverse_swaps_before_selection_and_dim() {
        let p = Palette::default();
        let inverse = Style {
            fg: Color::Rgb(10, 10, 10),
            bg: Color::Rgb(200, 200, 200),
            attrs: Attrs::INVERSE,
            ..Style::DEFAULT
        };
        let r = p.resolve_style(inverse, false);
        assert_eq!(r.fg, (200, 200, 200));
        assert_eq!(r.bg, (10, 10, 10));

        // Inverse on a default-background cell forces an explicit quad.
        let inverse_default = Style {
            attrs: Attrs::INVERSE,
            ..Style::DEFAULT
        };
        let r = p.resolve_style(inverse_default, false);
        assert_eq!(r.fg, p.bg);
        assert_eq!(r.bg, p.fg);
        assert!(!r.bg_is_default);

        // Dim applies to the post-swap foreground.
        let both = Style {
            attrs: Attrs::INVERSE | Attrs::DIM,
            fg: Color::Rgb(0, 0, 0),
            bg: Color::Rgb(100, 100, 100),
            ..Style::DEFAULT
        };
        assert_eq!(p.resolve_style(both, false).fg, (70, 70, 70));
    }

    #[test]
    fn selection_overrides_the_background_last() {
        let mut p = Palette::default();
        let style = Style {
            fg: Color::Rgb(1, 1, 1),
            bg: Color::Rgb(2, 2, 2),
            ..Style::DEFAULT
        };
        let r = p.resolve_style(style, true);
        assert_eq!(r.bg, p.selection_bg);
        assert_eq!(r.fg, (1, 1, 1));
        assert!(!r.bg_is_default);

        p.selection_fg = Some((9, 9, 9));
        assert_eq!(p.resolve_style(style, true).fg, (9, 9, 9));

        // Selection beats inverse, which already ran.
        let inverse = Style {
            attrs: Attrs::INVERSE,
            ..style
        };
        assert_eq!(p.resolve_style(inverse, true).bg, p.selection_bg);
        assert_eq!(p.resolve_style(inverse, true).fg, (9, 9, 9));
    }

    #[test]
    fn hidden_paints_the_glyph_in_the_background_colour() {
        let p = Palette::default();
        let hidden = Style {
            fg: Color::Rgb(255, 0, 0),
            bg: Color::Rgb(1, 2, 3),
            attrs: Attrs::HIDDEN,
            ..Style::DEFAULT
        };
        let r = p.resolve_style(hidden, false);
        assert_eq!(r.fg, r.bg);
        assert_eq!(r.fg, (1, 2, 3));
    }

    #[test]
    fn the_underline_colour_defaults_to_the_foreground() {
        let p = Palette::default();
        let plain = Style {
            fg: Color::Rgb(7, 7, 7),
            attrs: Attrs::UNDERLINE,
            ..Style::DEFAULT
        };
        assert_eq!(p.resolve_style(plain, false).underline, (7, 7, 7));

        let coloured = Style {
            underline_color: Color::Indexed(196),
            ..plain
        };
        assert_eq!(p.resolve_style(coloured, false).underline, (255, 0, 0));
        assert_eq!(p.resolve_style(coloured, false).attrs.underline_kind(), 0);
    }
}
