//! The `theme` prop: JSON from React → [`st_client_core::Palette`].
//!
//! `docs/plan/04-client-native.md` §10. React owns the config parser (the TOML
//! is read once, in TS), so the element never sees `config.toml`; it sees a
//! plain object and has to be forgiving about what is missing.

use st_client_core::palette::{Palette, Rgb, DEFAULT_ANSI};

/// The default dark palette from 04 §10: xterm-ish ANSI, `#1e1e1e` on `#d4d4d4`.
#[must_use]
pub fn default_palette() -> Palette {
    let mut palette = Palette::new(DEFAULT_ANSI, (0xd4, 0xd4, 0xd4), (0x1e, 0x1e, 0x1e));
    palette.cursor = (0xd4, 0xd4, 0xd4);
    palette.cursor_text = (0x1e, 0x1e, 0x1e);
    palette.selection_bg = (0x3a, 0x3d, 0x41);
    palette.selection_fg = None;
    palette.bold_is_bright = false;
    palette.rebuild();
    palette
}

/// Parses the `theme` prop. Unknown and malformed entries keep their default
/// rather than failing the frame: a half-typed theme in React must not blank
/// the terminal.
///
/// Accepted colour spellings: `"#rgb"`, `"#rrggbb"`, `"#rrggbbaa"` (alpha
/// ignored — a cell background is opaque or absent, never blended), the same
/// three without the `#`, and `[r, g, b]` with 0–255 numbers.
#[must_use]
pub fn palette_from_json(value: &serde_json::Value) -> Palette {
    let mut palette = default_palette();
    let Some(object) = value.as_object() else {
        return palette;
    };

    if let Some(list) = object.get("ansi").and_then(|v| v.as_array()) {
        let mut ansi = *palette.ansi();
        for (slot, entry) in ansi.iter_mut().zip(list.iter()) {
            if let Some(rgb) = parse_color(entry) {
                *slot = rgb;
            }
        }
        palette.set_ansi(ansi);
    }

    let set = |key: &str, slot: &mut Rgb| {
        if let Some(rgb) = object.get(key).and_then(parse_color) {
            *slot = rgb;
        }
    };
    set("fg", &mut palette.fg);
    set("bg", &mut palette.bg);
    set("cursor", &mut palette.cursor);
    set("cursorText", &mut palette.cursor_text);
    set("selectionBg", &mut palette.selection_bg);

    // `selectionFg: null` is meaningful — "keep each cell's own foreground" —
    // so absent and explicit-null both have to land on `None`.
    palette.selection_fg = object.get("selectionFg").and_then(parse_color);

    if let Some(flag) = object
        .get("boldIsBright")
        .and_then(serde_json::Value::as_bool)
    {
        palette.bold_is_bright = flag;
    }

    palette.rebuild();
    palette
}

/// One colour, in any of the accepted spellings.
#[must_use]
pub fn parse_color(value: &serde_json::Value) -> Option<Rgb> {
    if let Some(text) = value.as_str() {
        return parse_hex(text);
    }
    let list = value.as_array()?;
    if list.len() < 3 {
        return None;
    }
    let channel = |i: usize| -> Option<u8> {
        let n = list.get(i)?.as_f64()?;
        Some(n.round().clamp(0.0, 255.0) as u8)
    };
    Some((channel(0)?, channel(1)?, channel(2)?))
}

/// `#rgb`, `#rrggbb`, `#rrggbbaa`, with or without the `#`. Alpha is parsed and
/// discarded so a theme copied from CSS still loads.
#[must_use]
pub fn parse_hex(raw: &str) -> Option<Rgb> {
    let hex = raw.trim().trim_start_matches('#');
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let nibble = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok();
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        3 => Some((nibble(0)? * 17, nibble(1)? * 17, nibble(2)? * 17)),
        6 | 8 => Some((byte(0)?, byte(2)?, byte(4)?)),
        _ => None,
    }
}

/// `(u8, u8, u8)` → the opaque gpui colour the painter wants.
#[must_use]
pub fn rgba(rgb: Rgb) -> gpui::Rgba {
    gpui::Rgba {
        r: f32::from(rgb.0) / 255.0,
        g: f32::from(rgb.1) / 255.0,
        b: f32::from(rgb.2) / 255.0,
        a: 1.0,
    }
}

/// `(u8, u8, u8)` at an explicit alpha, for the hover scrollbar and the
/// unfocused cursor outline.
#[must_use]
pub fn rgba_with_alpha(rgb: Rgb, alpha: f32) -> gpui::Rgba {
    gpui::Rgba {
        a: alpha.clamp(0.0, 1.0),
        ..rgba(rgb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_every_accepted_hex_shape() {
        assert_eq!(parse_hex("#f00"), Some((255, 0, 0)));
        assert_eq!(parse_hex("00ff00"), Some((0, 255, 0)));
        assert_eq!(parse_hex("#0000ff80"), Some((0, 0, 255)));
        assert_eq!(parse_hex("  #123456  "), Some((0x12, 0x34, 0x56)));
        assert_eq!(parse_hex("rebeccapurple"), None);
        assert_eq!(parse_hex("#ggg"), None);
        assert_eq!(parse_hex("#12345"), None);
    }

    #[test]
    fn parses_array_colours_and_clamps_them() {
        assert_eq!(parse_color(&json!([1, 2, 3])), Some((1, 2, 3)));
        assert_eq!(parse_color(&json!([300, -4, 2.6])), Some((255, 0, 3)));
        assert_eq!(parse_color(&json!([1, 2])), None);
        assert_eq!(parse_color(&json!(7)), None);
    }

    #[test]
    fn a_partial_theme_keeps_the_defaults_for_everything_else() {
        let default = default_palette();
        let palette = palette_from_json(&json!({ "bg": "#000000" }));
        assert_eq!(palette.bg, (0, 0, 0));
        assert_eq!(palette.fg, default.fg);
        assert_eq!(palette.ansi(), default.ansi());
        assert!(!palette.bold_is_bright);
    }

    #[test]
    fn a_short_ansi_list_only_overrides_the_entries_it_has() {
        let palette = palette_from_json(&json!({ "ansi": ["#111111", "#222222"] }));
        assert_eq!(palette.ansi()[0], (0x11, 0x11, 0x11));
        assert_eq!(palette.ansi()[1], (0x22, 0x22, 0x22));
        assert_eq!(palette.ansi()[2], DEFAULT_ANSI[2]);
        // The 256-entry table has to be rebuilt, not just the 16 ANSI slots.
        assert_eq!(palette.indexed(0), (0x11, 0x11, 0x11));
    }

    #[test]
    fn selection_fg_null_means_keep_the_cells_own_foreground() {
        let palette = palette_from_json(&json!({ "selectionFg": serde_json::Value::Null }));
        assert_eq!(palette.selection_fg, None);
        let palette = palette_from_json(&json!({ "selectionFg": "#ffffff" }));
        assert_eq!(palette.selection_fg, Some((255, 255, 255)));
    }

    #[test]
    fn a_non_object_theme_is_the_default_palette() {
        assert_eq!(
            palette_from_json(&json!("dracula")).bg,
            default_palette().bg
        );
        assert_eq!(
            palette_from_json(&serde_json::Value::Null).fg,
            default_palette().fg
        );
    }

    #[test]
    fn bold_is_bright_reaches_style_resolution() {
        use st_proto::{Attrs, Color, Style};
        let bold_red = Style {
            fg: Color::Indexed(1),
            attrs: Attrs::BOLD,
            ..Style::DEFAULT
        };
        let plain = palette_from_json(&json!({}));
        assert_eq!(plain.resolve_style(bold_red, false).fg, plain.indexed(1));
        let bright = palette_from_json(&json!({ "boldIsBright": true }));
        assert_eq!(bright.resolve_style(bold_red, false).fg, bright.indexed(9));
    }
}
