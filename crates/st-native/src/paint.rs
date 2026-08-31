//! One frame of `<terminal-grid>` (04 §6).
//!
//! Paint order, and why it is this order:
//! 1. background quads — merged, and skipped entirely where the cell's
//!    background is the theme default so a blurred window shows through (Q28);
//! 2. glyph runs, through the shaped-line cache;
//! 3. the double-underline second stroke (gpui's `UnderlineStyle` has no
//!    `double`, so the first stroke rides on the `TextRun` and the second is a
//!    quad);
//! 4. the cursor, on top of its own cell;
//! 5. the scrollbar, in the right padding.
//!
//! The selection is *not* a separate overlay pass: `layout_row` resolves it
//! into each cell's style, which is what makes `selectionFg` work and keeps
//! selected text in the same run-grouping pass as everything else.
//!
//! **GPUI does not inherit text colour** (gpuix README, invariant I10): every
//! `TextRun` built here carries an explicit `color`, and nothing relies on an
//! ancestor `div().text_color()`.

use std::time::Instant;

use gpui::{fill, point, px, size, App, Bounds, ContentMask, Pixels, Point, ShapedLine, Window};
use st_client_core::palette::Rgb;
use st_proto::{AbsLine, Modes, SurfaceId};

use crate::element::GridState;
use crate::geometry::{
    scrollbar_thumb, CellSize, GridGeometry, SCROLLBAR_MIN_THUMB, SCROLLBAR_THUMB_WIDTH,
    SCROLLBAR_THUMB_WIDTH_HOVER, SCROLLBAR_WIDTH,
};
use crate::props::{CursorStyle, ScrollbarMode};
use crate::runs::{layout_viewport, RowLayout, RunSpan, StyleKey};
use crate::theme::{rgba, rgba_with_alpha};

/// How many history lines one `FetchHistory` asks for (04 §4).
pub const HISTORY_PAGE: u64 = 1000;
/// Cursor blink period (04 §6 step 6).
pub const BLINK_MS: u64 = 530;

/// Key for the shaped-line cache. The text and the whole style key, because a
/// palette swap changes the colour baked into the `TextRun`'s decoration runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunKey {
    /// The exact string that was shaped.
    pub text: String,
    /// Colour and decoration.
    pub style: StyleKey,
    /// Whether the advance was forced to the cell width.
    pub forced: bool,
}

/// What the painter read out of the Replica while holding the lock.
struct FrameData {
    rows: Vec<RowLayout>,
    cursor: Option<(u16, u16)>,
    cursor_style: CursorStyle,
    cursor_blink: bool,
    modes: Modes,
    title: String,
    content_lines: u64,
    viewport_top: u64,
    scroll_offset: u64,
    cols: u16,
    rows_painted: u16,
    replica_size: (u16, u16),
    missing_history: Option<(u64, u16)>,
}

/// Paints one frame into `bounds`.
///
/// Returns `false` when there is nothing to paint yet (no Surface attached),
/// which the caller uses to skip publishing stats for a frame that did no work.
pub fn paint_frame(
    state: &mut GridState,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let started = Instant::now();
    state.stats.begin_frame();
    state.bounds = Some(bounds);

    let cell = resolve_cell(state, cx);
    let geometry = GridGeometry::fit(
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
        cell,
        state.props.padding,
    );
    state.geometry = geometry;

    let Some(surface) = state.props.surface_id.map(SurfaceId) else {
        return false;
    };

    request_resize(state, surface, geometry);

    let Some(frame) = read_frame(state, surface, geometry) else {
        return false;
    };

    if let Some((from, count)) = frame.missing_history {
        request_history(state, surface, from, count);
    }

    paint_backgrounds(state, &frame, bounds, geometry, window);
    paint_glyphs(state, &frame, bounds, geometry, window, cx);
    paint_cursor(state, &frame, bounds, geometry, window, cx);
    paint_scrollbar(state, &frame, bounds, geometry, window);

    state.title = frame.title;
    state.modes = frame.modes;
    state.content_lines = frame.content_lines;
    state.viewport_top = frame.viewport_top;
    state.scroll_offset = frame.scroll_offset;
    state.stats.rows = frame.rows_painted;
    state.stats.cols = frame.cols;
    state.replica_size = frame.replica_size;
    state.stats.end_frame(started.elapsed());
    true
}

/// Cell metrics, recomputed only when the font key changes (04 §6).
fn resolve_cell(state: &mut GridState, cx: &mut App) -> CellSize {
    let key = state.props.font_key();
    if state.cell_font_key.as_ref() == Some(&key) {
        if let Some(cell) = state.cell {
            return cell;
        }
    }

    let font_size = px(state.props.font_size);
    let font_id = cx.text_system().resolve_font(&state.font(false, false));
    // 04 §6 and Zed's `terminal_element.rs` both take the advance of a
    // lowercase 'm'; on a monospace face every glyph shares it, and on a
    // fallback face it is the least-bad guess.
    let width = cx
        .text_system()
        .advance(font_id, font_size, 'm')
        .map_or(state.props.font_size * 0.6, |advance| {
            f32::from(advance.width)
        });
    let cell = CellSize::new(width, state.props.font_size * state.props.line_height);

    state.cell = Some(cell);
    state.cell_font_key = Some(key);
    // Both the glyph geometry and the forced advance are baked into a cached
    // shaped line, so none of them survive a font change.
    state.run_cache.clear();
    cell
}

/// Tells the Server about a new grid size, once per size (04 §6).
fn request_resize(state: &mut GridState, surface: SurfaceId, geometry: GridGeometry) {
    let size = (geometry.cols, geometry.rows);
    if state.last_sent_size == Some(size) || state.replica_size == size {
        return;
    }
    let Some(handle) = &state.handle else {
        return;
    };
    if handle.resize(surface, size.0, size.1).is_ok() {
        state.last_sent_size = Some(size);
        state.emit_resize(size.0, size.1);
    }
}

/// Asks for a page of scrollback, one outstanding request at a time (Q25).
fn request_history(state: &mut GridState, surface: SurfaceId, from: u64, count: u16) {
    if state.pending_history == Some(from) {
        return;
    }
    let Some(handle) = &state.handle else {
        return;
    };
    if handle
        .fetch_history(surface, AbsLine::new(from), count)
        .is_ok()
    {
        state.pending_history = Some(from);
    }
}

/// Copies the frame out of the Replica.
///
/// The lock is held across the run-grouping pass, not just a memcpy as 04 §6
/// suggests: grouping needs the `StyleTable`, and cloning a 4 096-entry table
/// every frame costs far more than the few microseconds the pass takes over
/// `cols × rows` cells. The Data Plane thread's only other work under this
/// lock is applying a Delta, which is the same order of magnitude.
fn read_frame(
    state: &mut GridState,
    surface: SurfaceId,
    geometry: GridGeometry,
) -> Option<FrameData> {
    let selection = state.selection;
    let palette = &state.props.palette;
    let scroll_offset = state.scroll_offset;
    let fallback_style = state.props.cursor_style;
    let fallback_blink = state.props.cursor_blink;

    let handle = state.handle.as_ref()?;
    let frame = handle.with_replica(surface, |replica| {
        let replica_size = (replica.cols(), replica.rows());
        // Q40: a Replica that has not caught up with the resize is letterboxed
        // rather than reflowed; we paint the intersection.
        let cols = geometry.cols.min(replica.cols());
        let rows = geometry.rows.min(replica.rows());
        let offset = scroll_offset.min(replica.max_scroll_offset());
        let range = replica.viewport_range(offset);

        // Lines the local history cache has not reached yet come out blank and
        // are fetched below; the frame is never held up on the network.
        let rows_out = layout_viewport(
            replica,
            cols,
            rows,
            range.start,
            selection.as_ref(),
            palette,
        );

        let cursor = replica.cursor();
        let cursor_visible =
            replica.cursor_visible_at(offset) && cursor.row < rows && cursor.col < cols;

        let cached = replica.cached_history_range();
        let missing_history = crate::geometry::missing_history_page(
            range.start.get(),
            cached.start,
            replica.history_base().get(),
            HISTORY_PAGE,
        );

        FrameData {
            rows: rows_out,
            cursor: cursor_visible.then_some((cursor.col, cursor.row)),
            cursor_style: if cursor.shape == st_proto::CursorShape::default() {
                fallback_style
            } else {
                CursorStyle::from_wire(cursor.shape)
            },
            cursor_blink: cursor.blink && fallback_blink,
            modes: replica.modes(),
            title: replica.title().to_string(),
            content_lines: replica.total_lines(),
            viewport_top: range.start.get(),
            scroll_offset: offset,
            cols,
            rows_painted: rows,
            replica_size,
            missing_history,
        }
    });

    // Clearing the dirty flag *after* reading is what makes a Delta that lands
    // mid-frame schedule the next one instead of being lost (Q27).
    let _ = handle.take_dirty();
    frame
}

fn paint_backgrounds(
    state: &mut GridState,
    frame: &FrameData,
    bounds: Bounds<Pixels>,
    geometry: GridGeometry,
    window: &mut Window,
) {
    let mut quads = 0u32;
    for (row_index, row) in frame.rows.iter().enumerate() {
        for span in &row.backgrounds {
            let (x, y) = geometry.cell_origin(span.col, row_index as u16);
            let quad_bounds = Bounds::new(
                point(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                size(
                    px((geometry.cell.width * f32::from(span.cells)).ceil()),
                    px(geometry.cell.height.ceil()),
                ),
            );
            window.paint_quad(fill(quad_bounds, rgba(span.color)));
            quads += 1;
        }
    }
    state.stats.bg_quads = quads;
}

fn paint_glyphs(
    state: &mut GridState,
    frame: &FrameData,
    bounds: Bounds<Pixels>,
    geometry: GridGeometry,
    window: &mut Window,
    cx: &mut App,
) {
    let font_size = px(state.props.font_size);
    let line_height = px(geometry.cell.height);
    let mut shaped = 0u32;
    let mut cached = 0u32;

    for row_index in 0..frame.rows.len() {
        for run_index in 0..frame.rows[row_index].runs.len() {
            let run = frame.rows[row_index].runs[run_index].clone();
            let (x, y) = geometry.cell_origin(run.col, row_index as u16);
            let origin = point(bounds.origin.x + px(x), bounds.origin.y + px(y));
            let line = shaped_line(
                state,
                &run,
                geometry.cell,
                font_size,
                window,
                &mut shaped,
                &mut cached,
            );

            // A wide glyph or an emoji from a fallback face can overshoot its
            // cells; clip it so it never bleeds into the next column.
            let mask = ContentMask {
                bounds: Bounds::new(
                    origin,
                    size(px(geometry.cell.width * f32::from(run.cells)), line_height),
                ),
            };
            window.with_content_mask(Some(mask), |window| {
                let _ = line.paint(origin, line_height, gpui::TextAlign::Left, None, window, cx);
            });

            if run.key.underline == 2 {
                paint_double_underline(&run, origin, geometry, line_height, window);
            }
        }
    }

    state.stats.shaped_runs = shaped;
    state.stats.cached_runs = cached;
}

/// Fetches or builds the shaped line for one run.
fn shaped_line(
    state: &mut GridState,
    run: &RunSpan,
    cell: CellSize,
    font_size: Pixels,
    window: &mut Window,
    shaped: &mut u32,
    cached: &mut u32,
) -> ShapedLine {
    let key = RunKey {
        text: run.text.clone(),
        style: run.key,
        forced: !run.wide,
    };
    if let Some(line) = state.run_cache.get(&key) {
        *cached += 1;
        return line.clone();
    }
    let text_run = gpui::TextRun {
        len: run.text.len(),
        font: state.font(run.key.bold, run.key.italic),
        color: rgba(run.key.fg).into(),
        // The quad pass already painted the background; a second one here
        // would double-blend on a transparent window.
        background_color: None,
        underline: underline_style(run.key),
        strikethrough: run.key.strike.then(|| gpui::StrikethroughStyle {
            thickness: px(1.0),
            color: Some(rgba(run.key.fg).into()),
        }),
    };
    let line = window.text_system().shape_line(
        run.text.clone().into(),
        font_size,
        std::slice::from_ref(&text_run),
        // A wide char is one glyph in two cells; forcing it to one cell would
        // squash it and shift everything after it.
        (!run.wide).then(|| px(cell.width)),
    );
    state.run_cache.insert(key, line.clone());
    *shaped += 1;
    line
}

/// gpui has straight and wavy; the terminal has five kinds. Curly maps to
/// wavy, everything else to a straight stroke, and `double` gets a second
/// stroke painted as a quad by the caller.
fn underline_style(key: StyleKey) -> Option<gpui::UnderlineStyle> {
    (key.underline != 0).then(|| gpui::UnderlineStyle {
        thickness: px(1.0),
        color: Some(rgba(key.underline_color).into()),
        wavy: key.underline == 3,
    })
}

fn paint_double_underline(
    run: &RunSpan,
    origin: Point<Pixels>,
    geometry: GridGeometry,
    line_height: Pixels,
    window: &mut Window,
) {
    let y = origin.y + line_height - px(2.0);
    let quad = Bounds::new(
        point(origin.x, y),
        size(px(geometry.cell.width * f32::from(run.cells)), px(1.0)),
    );
    window.paint_quad(fill(quad, rgba(run.key.underline_color)));
}

fn paint_cursor(
    state: &mut GridState,
    frame: &FrameData,
    bounds: Bounds<Pixels>,
    geometry: GridGeometry,
    window: &mut Window,
    cx: &mut App,
) {
    let Some((col, row)) = frame.cursor else {
        return;
    };
    let focused = state.is_focused(window);
    // 04 §6 step 6: blink only while focused; an unfocused terminal costs no
    // frames and paints a steady outline instead.
    if focused && frame.cursor_blink && !state.blink_on() {
        return;
    }

    let (x, y) = geometry.cell_origin(col, row);
    let origin = point(bounds.origin.x + px(x), bounds.origin.y + px(y));
    let (cursor_color, text_color) = state.props.palette.cursor_colors();
    let cell_bounds = Bounds::new(
        origin,
        size(px(geometry.cell.width), px(geometry.cell.height)),
    );

    if !focused {
        paint_outline(cell_bounds, cursor_color, window);
        return;
    }

    match frame.cursor_style {
        CursorStyle::Block => {
            window.paint_quad(fill(cell_bounds, rgba(cursor_color)));
            repaint_cursor_glyph(
                state, frame, col, row, origin, geometry, text_color, window, cx,
            );
        }
        CursorStyle::Beam => {
            let beam = Bounds::new(origin, size(px(2.0), px(geometry.cell.height)));
            window.paint_quad(fill(beam, rgba(cursor_color)));
        }
        CursorStyle::Underline => {
            let bar = Bounds::new(
                point(origin.x, origin.y + px(geometry.cell.height - 2.0)),
                size(px(geometry.cell.width), px(2.0)),
            );
            window.paint_quad(fill(bar, rgba(cursor_color)));
        }
    }
}

/// The glyph a block cursor covers, repainted in `cursorText` (04 §6).
#[allow(clippy::too_many_arguments)]
fn repaint_cursor_glyph(
    state: &mut GridState,
    frame: &FrameData,
    col: u16,
    row: u16,
    origin: Point<Pixels>,
    geometry: GridGeometry,
    text_color: Rgb,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(layout) = frame.rows.get(usize::from(row)) else {
        return;
    };
    let Some(text) = glyph_at(layout, col) else {
        return;
    };
    let text_run = gpui::TextRun {
        len: text.len(),
        font: state.font(false, false),
        color: rgba(text_color).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let font_size = px(state.props.font_size);
    let line = window.text_system().shape_line(
        text.into(),
        font_size,
        std::slice::from_ref(&text_run),
        Some(px(geometry.cell.width)),
    );
    let _ = line.paint(
        origin,
        px(geometry.cell.height),
        gpui::TextAlign::Left,
        None,
        window,
        cx,
    );
}

/// The character painted at `col`, walking the row's runs.
fn glyph_at(layout: &RowLayout, col: u16) -> Option<String> {
    for run in &layout.runs {
        if col < run.col || col >= run.col + run.cells {
            continue;
        }
        let mut offset = usize::from(col - run.col);
        if run.wide {
            offset = 0;
        }
        return run.text.chars().nth(offset).map(String::from);
    }
    None
}

/// A 1 px hollow block, the unfocused cursor (04 §6).
fn paint_outline(bounds: Bounds<Pixels>, color: Rgb, window: &mut Window) {
    let thickness = px(1.0);
    let color = rgba_with_alpha(color, 0.85);
    let top = Bounds::new(bounds.origin, size(bounds.size.width, thickness));
    let bottom = Bounds::new(
        point(
            bounds.origin.x,
            bounds.origin.y + bounds.size.height - thickness,
        ),
        size(bounds.size.width, thickness),
    );
    let left = Bounds::new(bounds.origin, size(thickness, bounds.size.height));
    let right = Bounds::new(
        point(
            bounds.origin.x + bounds.size.width - thickness,
            bounds.origin.y,
        ),
        size(thickness, bounds.size.height),
    );
    for edge in [top, bottom, left, right] {
        window.paint_quad(fill(edge, color));
    }
}

fn paint_scrollbar(
    state: &mut GridState,
    frame: &FrameData,
    bounds: Bounds<Pixels>,
    geometry: GridGeometry,
    window: &mut Window,
) {
    state.scrollbar_thumb = None;
    if state.props.scrollbar == ScrollbarMode::Never {
        return;
    }
    if state.props.scrollbar == ScrollbarMode::Auto && !state.pointer_recently_moved() {
        // Still compute the thumb so a click in the padding is hit-tested
        // consistently; just do not paint it.
        state.scrollbar_thumb = scrollbar_thumb(
            f32::from(bounds.size.height),
            frame.rows_painted,
            frame.content_lines,
            frame.scroll_offset,
        );
        return;
    }

    let track_height = f32::from(bounds.size.height);
    let Some(thumb) = scrollbar_thumb(
        track_height,
        frame.rows_painted,
        frame.content_lines,
        frame.scroll_offset,
    ) else {
        return;
    };
    state.scrollbar_thumb = Some(thumb);

    let width = if state.scrollbar_hover {
        SCROLLBAR_THUMB_WIDTH_HOVER
    } else {
        SCROLLBAR_THUMB_WIDTH
    };
    let x = bounds.origin.x + px(geometry.scrollbar_x()) + px(SCROLLBAR_WIDTH - width);
    let quad = Bounds::new(
        point(x, bounds.origin.y + px(thumb.y)),
        size(
            px(width),
            px(thumb.height.max(SCROLLBAR_MIN_THUMB.min(track_height))),
        ),
    );
    window.paint_quad(fill(quad, rgba_with_alpha(state.props.palette.fg, 0.35)));
}

/// Re-exported for the element's hit testing.
pub use crate::geometry::ScrollbarThumb as Thumb;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::RunSpan;

    fn run(col: u16, cells: u16, text: &str, wide: bool) -> RunSpan {
        RunSpan {
            col,
            cells,
            text: text.to_string(),
            key: StyleKey {
                fg: (255, 255, 255),
                underline_color: (255, 255, 255),
                bold: false,
                italic: false,
                underline: 0,
                strike: false,
            },
            wide,
        }
    }

    #[test]
    fn the_cursor_glyph_is_found_by_column() {
        let layout = RowLayout {
            backgrounds: Vec::new(),
            runs: vec![run(0, 5, "hello", false), run(6, 3, "abc", false)],
        };
        assert_eq!(glyph_at(&layout, 0).as_deref(), Some("h"));
        assert_eq!(glyph_at(&layout, 4).as_deref(), Some("o"));
        assert_eq!(glyph_at(&layout, 5), None, "a gap has no glyph");
        assert_eq!(glyph_at(&layout, 7).as_deref(), Some("b"));
        assert_eq!(glyph_at(&layout, 99), None);
    }

    #[test]
    fn both_cells_of_a_wide_char_report_the_same_glyph() {
        let layout = RowLayout {
            backgrounds: Vec::new(),
            runs: vec![run(2, 2, "世", true)],
        };
        assert_eq!(glyph_at(&layout, 2).as_deref(), Some("世"));
        assert_eq!(glyph_at(&layout, 3).as_deref(), Some("世"));
    }

    #[test]
    fn a_run_key_separates_forced_and_natural_advances() {
        let style = run(0, 1, "a", false).key;
        let forced = RunKey {
            text: "a".to_string(),
            style,
            forced: true,
        };
        let natural = RunKey {
            forced: false,
            ..forced.clone()
        };
        assert_ne!(forced, natural);
    }

    #[test]
    fn the_underline_kind_maps_onto_what_gpui_can_draw() {
        let mut key = run(0, 1, "a", false).key;
        assert!(underline_style(key).is_none());
        key.underline = 1;
        assert!(underline_style(key).is_some_and(|style| !style.wavy));
        key.underline = 3;
        assert!(underline_style(key).is_some_and(|style| style.wavy));
        key.underline = 2;
        assert!(underline_style(key).is_some_and(|style| !style.wavy));
    }
}
