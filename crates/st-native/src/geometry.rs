//! Cell metrics, grid sizing, scroll arithmetic and scrollbar geometry.
//!
//! Everything in here is pure `f32`/`u64` maths with no gpui types, because
//! this is the half of the painter that can be tested on a box with no GPU
//! (`docs/plan/06-testing-perf-ci.md`, and grilling Q47: Linux is the primary
//! development platform for everything that is not a GPU test).

use st_client_core::selection::CellMetrics;

use crate::props::Padding;

/// One cell's box, derived from the font once per font change (04 §6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellSize {
    /// Advance width of `'m'` in the resolved font at the resolved size.
    pub width: f32,
    /// `font_size * line_height`.
    pub height: f32,
}

impl CellSize {
    /// A cell, guarded against the degenerate zero that a missing glyph or a
    /// zero font size would produce — every consumer divides by these.
    #[must_use]
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width: if width.is_finite() && width > 0.0 {
                width
            } else {
                1.0
            },
            height: if height.is_finite() && height > 0.0 {
                height
            } else {
                1.0
            },
        }
    }
}

/// The pixel geometry of one painted frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridGeometry {
    /// Cell box.
    pub cell: CellSize,
    /// Inset inside the element bounds.
    pub padding: Padding,
    /// Columns that fit.
    pub cols: u16,
    /// Rows that fit.
    pub rows: u16,
    /// Width of the element.
    pub width: f32,
    /// Height of the element.
    pub height: f32,
}

/// Widest grid we will ever ask the server for. 02 §4 caps a Row at `u16`
/// columns; this keeps a silly window size from asking for a 60 000-column PTY.
pub const MAX_COLS: u16 = 1000;
/// Same, vertically.
pub const MAX_ROWS: u16 = 1000;

impl GridGeometry {
    /// Fits a grid into `width × height` (04 §6):
    /// `cols = floor((w - pad.l - pad.r) / cell_w)`, likewise for rows.
    ///
    /// Always at least 1×1: a zero-sized grid makes `Resize` meaningless and
    /// every downstream index a special case.
    #[must_use]
    pub fn fit(width: f32, height: f32, cell: CellSize, padding: Padding) -> Self {
        let usable_w = (width - padding.left - padding.right).max(0.0);
        let usable_h = (height - padding.top - padding.bottom).max(0.0);
        let cols = ((usable_w / cell.width).floor() as i64).clamp(1, i64::from(MAX_COLS)) as u16;
        let rows = ((usable_h / cell.height).floor() as i64).clamp(1, i64::from(MAX_ROWS)) as u16;
        Self {
            cell,
            padding,
            cols,
            rows,
            width,
            height,
        }
    }

    /// Top-left of a cell, in element-local pixels.
    #[must_use]
    pub fn cell_origin(&self, col: u16, row: u16) -> (f32, f32) {
        (
            self.padding.left + f32::from(col) * self.cell.width,
            self.padding.top + f32::from(row) * self.cell.height,
        )
    }

    /// The metrics `st_client_core::selection::hit_test` wants.
    #[must_use]
    pub fn hit_metrics(&self) -> CellMetrics {
        CellMetrics {
            cell_width: self.cell.width,
            line_height: self.cell.height,
            pad_left: self.padding.left,
            pad_top: self.padding.top,
        }
    }

    /// The x at which the scrollbar track starts. The track lives in the right
    /// padding, so a click right of this is a scrollbar click, not a cell one.
    #[must_use]
    pub fn scrollbar_x(&self) -> f32 {
        (self.width - SCROLLBAR_WIDTH).max(0.0)
    }
}

/// Track width in px. The thumb is narrower until hovered (04 §6).
pub const SCROLLBAR_WIDTH: f32 = 10.0;
/// Idle thumb width.
pub const SCROLLBAR_THUMB_WIDTH: f32 = 6.0;
/// Hovered thumb width.
pub const SCROLLBAR_THUMB_WIDTH_HOVER: f32 = 10.0;
/// A thumb shorter than this is unclickable.
pub const SCROLLBAR_MIN_THUMB: f32 = 24.0;
/// How long after the last pointer movement `scrollbar: "auto"` keeps painting.
pub const SCROLLBAR_FADE_MS: u64 = 1500;

/// Vertical placement of the scrollbar thumb.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarThumb {
    /// Offset from the top of the track, in px.
    pub y: f32,
    /// Thumb height, in px.
    pub height: f32,
}

/// Thumb geometry for `scroll_offset` lines from the bottom of `content_lines`
/// with `rows` on screen. `None` when everything fits.
#[must_use]
pub fn scrollbar_thumb(
    track_height: f32,
    rows: u16,
    content_lines: u64,
    scroll_offset: u64,
) -> Option<ScrollbarThumb> {
    let rows = u64::from(rows);
    if rows == 0 || content_lines <= rows || track_height <= 0.0 {
        return None;
    }
    let visible_fraction = rows as f32 / content_lines as f32;
    let height = (track_height * visible_fraction)
        .clamp(SCROLLBAR_MIN_THUMB.min(track_height), track_height);
    let max_offset = content_lines - rows;
    // `scroll_offset` counts *up* from the bottom, the thumb counts down from
    // the top, so the two are mirrored.
    let from_top = max_offset.saturating_sub(scroll_offset.min(max_offset)) as f32;
    let travel = track_height - height;
    let y = if max_offset == 0 {
        0.0
    } else {
        travel * (from_top / max_offset as f32)
    };
    Some(ScrollbarThumb {
        y: y.clamp(0.0, travel.max(0.0)),
        height,
    })
}

/// The scroll offset a thumb dragged to `y` (its top, in track pixels) means.
#[must_use]
pub fn scroll_offset_for_thumb(
    track_height: f32,
    thumb_height: f32,
    rows: u16,
    content_lines: u64,
    y: f32,
) -> u64 {
    let rows = u64::from(rows);
    if content_lines <= rows {
        return 0;
    }
    let max_offset = content_lines - rows;
    let travel = track_height - thumb_height;
    if travel <= 0.0 {
        return 0;
    }
    let fraction = (y / travel).clamp(0.0, 1.0);
    let from_top = (fraction * max_offset as f32).round() as u64;
    max_offset.saturating_sub(from_top.min(max_offset))
}

/// Applies a scroll of `delta` lines (positive = towards history) to an offset
/// measured *from the bottom* (04 §8, grilling Q25).
#[must_use]
pub fn scroll_by(offset: u64, delta: i64, max_offset: u64) -> u64 {
    let next = if delta >= 0 {
        offset.saturating_add(delta.unsigned_abs())
    } else {
        offset.saturating_sub(delta.unsigned_abs())
    };
    next.min(max_offset)
}

/// Where the viewport goes when the content grows underneath it.
///
/// Following the bottom (`offset == 0`) keeps following. Scrolled up, the same
/// absolute lines stay on screen, which — with the offset measured from the
/// bottom — means it is only re-clamped, never moved.
#[must_use]
pub fn after_content_growth(offset: u64, max_offset: u64) -> u64 {
    if offset == 0 {
        0
    } else {
        offset.min(max_offset)
    }
}

/// The history page to request when the viewport reaches above what the
/// Replica has cached (04 §4, Q25). `None` when nothing is missing.
///
/// Pages are aligned to `page` lines so repeated small scrolls re-request the
/// same page instead of walking a one-line window up the scrollback.
#[must_use]
pub fn missing_history_page(
    viewport_top: u64,
    cached_from: u64,
    history_base: u64,
    page: u64,
) -> Option<(u64, u16)> {
    if page == 0 || viewport_top >= cached_from || cached_from <= history_base {
        return None;
    }
    let want_end = cached_from;
    let want_start = viewport_top.saturating_sub(page / 2).max(history_base);
    let count = (want_end - want_start).min(page);
    if count == 0 {
        return None;
    }
    let from = want_end - count;
    Some((from, u16::try_from(count).unwrap_or(u16::MAX)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> CellSize {
        CellSize::new(8.0, 17.0)
    }

    fn no_padding() -> Padding {
        Padding {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }
    }

    #[test]
    fn a_grid_floors_to_whole_cells() {
        let geometry = GridGeometry::fit(647.0, 340.0, cell(), no_padding());
        assert_eq!(geometry.cols, 80);
        assert_eq!(geometry.rows, 20);
    }

    #[test]
    fn padding_comes_out_of_the_usable_area() {
        let padding = Padding {
            top: 4.0,
            right: 4.0,
            bottom: 4.0,
            left: 6.0,
        };
        let geometry = GridGeometry::fit(650.0, 348.0, cell(), padding);
        assert_eq!(geometry.cols, ((650.0 - 10.0) / 8.0f32).floor() as u16);
        assert_eq!(geometry.rows, ((348.0 - 8.0) / 17.0f32).floor() as u16);
        assert_eq!(geometry.cell_origin(0, 0), (6.0, 4.0));
        assert_eq!(geometry.cell_origin(2, 3), (6.0 + 16.0, 4.0 + 51.0));
    }

    #[test]
    fn a_grid_is_never_smaller_than_one_cell() {
        let geometry = GridGeometry::fit(0.0, 0.0, cell(), no_padding());
        assert_eq!((geometry.cols, geometry.rows), (1, 1));
        let squashed = GridGeometry::fit(4.0, 4.0, cell(), Padding::default());
        assert_eq!((squashed.cols, squashed.rows), (1, 1));
    }

    #[test]
    fn a_degenerate_cell_cannot_divide_by_zero() {
        let cell = CellSize::new(0.0, f32::NAN);
        assert_eq!(cell.width, 1.0);
        assert_eq!(cell.height, 1.0);
        let geometry = GridGeometry::fit(100.0, 100.0, cell, no_padding());
        assert_eq!((geometry.cols, geometry.rows), (100, 100));
    }

    #[test]
    fn a_huge_window_is_clamped_to_a_sane_pty_size() {
        let geometry = GridGeometry::fit(1_000_000.0, 1_000_000.0, cell(), no_padding());
        assert_eq!(geometry.cols, MAX_COLS);
        assert_eq!(geometry.rows, MAX_ROWS);
    }

    #[test]
    fn scrolling_clamps_at_both_ends() {
        assert_eq!(scroll_by(0, -3, 100), 0);
        assert_eq!(scroll_by(0, 3, 100), 3);
        assert_eq!(scroll_by(98, 5, 100), 100);
        assert_eq!(scroll_by(100, -1, 100), 99);
        assert_eq!(scroll_by(5, i64::MIN, 100), 0);
        assert_eq!(scroll_by(5, i64::MAX, 100), 100);
    }

    #[test]
    fn following_the_bottom_keeps_following_and_scrolled_up_stays_put() {
        assert_eq!(after_content_growth(0, 500), 0);
        assert_eq!(after_content_growth(42, 500), 42);
        // Eviction shrank the scrollback under us: re-clamp, do not wrap.
        assert_eq!(after_content_growth(600, 500), 500);
    }

    #[test]
    fn no_thumb_when_everything_fits() {
        assert!(scrollbar_thumb(200.0, 24, 24, 0).is_none());
        assert!(scrollbar_thumb(200.0, 24, 10, 0).is_none());
        assert!(scrollbar_thumb(0.0, 24, 1000, 0).is_none());
    }

    #[test]
    fn the_thumb_sits_at_the_bottom_when_following() {
        let track = 400.0;
        let thumb = scrollbar_thumb(track, 25, 100, 0).unwrap();
        assert!((thumb.height - 100.0).abs() < 0.01, "{thumb:?}");
        assert!((thumb.y + thumb.height - track).abs() < 0.01, "{thumb:?}");
    }

    #[test]
    fn the_thumb_sits_at_the_top_when_fully_scrolled_back() {
        let thumb = scrollbar_thumb(400.0, 25, 100, 75).unwrap();
        assert!(thumb.y.abs() < 0.01, "{thumb:?}");
    }

    #[test]
    fn the_thumb_never_shrinks_below_the_clickable_minimum() {
        let thumb = scrollbar_thumb(400.0, 25, 1_000_000, 0).unwrap();
        assert!(thumb.height >= SCROLLBAR_MIN_THUMB);
        assert!(thumb.y + thumb.height <= 400.001);
    }

    #[test]
    fn dragging_the_thumb_round_trips_to_the_offset_it_came_from() {
        let (track, rows, content) = (400.0, 25u16, 300u64);
        for offset in [0u64, 1, 37, 137, 274, 275] {
            let thumb = scrollbar_thumb(track, rows, content, offset).unwrap();
            let back = scroll_offset_for_thumb(track, thumb.height, rows, content, thumb.y);
            assert!(
                back.abs_diff(offset) <= 1,
                "offset {offset} -> y {} -> {back}",
                thumb.y
            );
        }
    }

    #[test]
    fn history_paging_asks_only_for_what_is_missing() {
        // Cache starts at 500, the viewport wants 480: fetch the page below 500.
        let (from, count) = missing_history_page(480, 500, 0, 1000).unwrap();
        assert!(from < 500 && u64::from(count) == 500 - from);
        // Nothing missing.
        assert!(missing_history_page(500, 500, 0, 1000).is_none());
        assert!(missing_history_page(900, 500, 0, 1000).is_none());
        // The server has nothing older than history_base.
        assert!(missing_history_page(0, 100, 100, 1000).is_none());
    }

    #[test]
    fn a_history_page_never_reaches_below_the_servers_oldest_line() {
        let (from, count) = missing_history_page(120, 300, 100, 1000).unwrap();
        assert_eq!(from, 100);
        assert_eq!(u64::from(count), 200);
    }

    #[test]
    fn a_history_page_is_capped_at_the_page_size() {
        let (from, count) = missing_history_page(0, 100_000, 0, 1000).unwrap();
        assert_eq!(u64::from(count), 1000);
        assert_eq!(from, 99_000);
    }
}
