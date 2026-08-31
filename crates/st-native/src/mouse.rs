//! Pointer glue (04 §8).
//!
//! Encoding and the wheel policy live in `st_client_core::mouse`; what is here
//! is the part that only the renderer knows: gpui's button and modifier enums,
//! turning a pixel-precise trackpad delta into whole lines without losing the
//! remainder, and throttling motion reports to one per cell.

use st_client_core::mouse::MouseButton;

/// Where a press landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitZone {
    /// Inside the grid.
    Cell,
    /// On the scrollbar thumb.
    ScrollbarThumb,
    /// On the scrollbar track, above or below the thumb.
    ScrollbarTrack,
}

/// Which part of the element the pointer is over.
///
/// The scrollbar is painted inside the right padding, so it has to be tested
/// before the cell grid or a click on the thumb would start a selection
/// (04 §8).
#[must_use]
pub fn hit_zone(
    x: f32,
    y: f32,
    scrollbar_x: f32,
    scrollbar_visible: bool,
    thumb: Option<crate::geometry::ScrollbarThumb>,
) -> HitZone {
    if !scrollbar_visible || x < scrollbar_x {
        return HitZone::Cell;
    }
    match thumb {
        Some(thumb) if y >= thumb.y && y <= thumb.y + thumb.height => HitZone::ScrollbarThumb,
        _ => HitZone::ScrollbarTrack,
    }
}

/// gpui's button enum → the protocol's.
///
/// `Navigate` (back/forward) has no xterm encoding, so it is dropped rather
/// than mapped onto something a program would misread.
#[must_use]
pub fn button_from_gpui(button: gpui::MouseButton) -> Option<MouseButton> {
    match button {
        gpui::MouseButton::Left => Some(MouseButton::Left),
        gpui::MouseButton::Middle => Some(MouseButton::Middle),
        gpui::MouseButton::Right => Some(MouseButton::Right),
        gpui::MouseButton::Navigate(_) => None,
    }
}

/// Turns a gpui scroll delta into whole lines, keeping the sub-line remainder.
///
/// A trackpad reports a few pixels per event; rounding each event to zero
/// lines would make the terminal ignore slow scrolling entirely, and rounding
/// each up would make it fly. Accumulate instead, exactly like every native
/// scroll view.
#[derive(Debug, Clone, Copy, Default)]
pub struct WheelAccumulator {
    residual: f32,
}

impl WheelAccumulator {
    /// Feeds a pixel-precise delta (trackpad). Positive `dy` is a scroll
    /// *towards history*, matching gpui's "content moves down" sign.
    #[must_use]
    pub fn push_pixels(&mut self, dy: f32, line_height: f32) -> i32 {
        if !dy.is_finite() || line_height <= 0.0 {
            return 0;
        }
        self.residual += dy / line_height;
        let whole = self.residual.trunc();
        self.residual -= whole;
        whole as i32
    }

    /// Feeds a line-based delta (a notch of a real wheel), scaled by
    /// `lines_per_notch`.
    #[must_use]
    pub fn push_lines(&mut self, dy: f32, lines_per_notch: u16) -> i32 {
        if !dy.is_finite() {
            return 0;
        }
        self.residual += dy * f32::from(lines_per_notch);
        let whole = self.residual.trunc();
        self.residual -= whole;
        whole as i32
    }

    /// Drops the remainder. Called when the gesture ends or the surface
    /// changes, so a half-line does not leak into the next scroll.
    pub fn reset(&mut self) {
        self.residual = 0.0;
    }
}

/// Suppresses motion reports until the pointer changes cell (04 §8).
#[derive(Debug, Clone, Copy, Default)]
pub struct MotionThrottle {
    last: Option<(u16, u16)>,
}

impl MotionThrottle {
    /// `true` when this cell should produce a report.
    #[must_use]
    pub fn should_report(&mut self, cell: (u16, u16)) -> bool {
        if self.last == Some(cell) {
            return false;
        }
        self.last = Some(cell);
        true
    }

    /// Forgets the last cell, so the next motion always reports.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// Bookkeeping for a click streak, so double- and triple-clicks pick the
/// selection mode (04 §8).
///
/// gpui already counts clicks in `MouseDownEvent::click_count`, but it keeps
/// counting past three; terminals cycle char → word → line → char.
#[must_use]
pub fn selection_mode_for(click_count: usize, alt: bool) -> st_client_core::SelectionMode {
    let count = if click_count == 0 {
        1
    } else {
        (((click_count - 1) % 3) + 1) as u8
    };
    st_client_core::SelectionMode::from_click(count, alt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::ScrollbarThumb;
    use st_client_core::SelectionMode;

    #[test]
    fn a_trackpad_dribble_accumulates_into_whole_lines() {
        let mut accumulator = WheelAccumulator::default();
        // Six 3-pixel events on a 17 px line: the first five round to zero,
        // and the sixth crosses the line boundary.
        let mut total = 0;
        for _ in 0..6 {
            total += accumulator.push_pixels(3.0, 17.0);
        }
        assert_eq!(total, 1);
    }

    #[test]
    fn accumulating_downwards_works_the_same() {
        let mut accumulator = WheelAccumulator::default();
        let mut total = 0;
        for _ in 0..12 {
            total += accumulator.push_pixels(-3.0, 17.0);
        }
        assert_eq!(total, -2);
    }

    #[test]
    fn a_wheel_notch_is_three_lines_by_default() {
        let mut accumulator = WheelAccumulator::default();
        assert_eq!(accumulator.push_lines(1.0, 3), 3);
        assert_eq!(accumulator.push_lines(-1.0, 3), -3);
    }

    #[test]
    fn a_reset_drops_the_remainder() {
        let mut accumulator = WheelAccumulator::default();
        assert_eq!(accumulator.push_pixels(8.0, 17.0), 0);
        accumulator.reset();
        assert_eq!(accumulator.push_pixels(8.0, 17.0), 0);
        assert_eq!(accumulator.push_pixels(9.1, 17.0), 1);
    }

    #[test]
    fn a_nonsense_delta_scrolls_nothing() {
        let mut accumulator = WheelAccumulator::default();
        assert_eq!(accumulator.push_pixels(f32::NAN, 17.0), 0);
        assert_eq!(accumulator.push_pixels(10.0, 0.0), 0);
    }

    #[test]
    fn motion_reports_once_per_cell() {
        let mut throttle = MotionThrottle::default();
        assert!(throttle.should_report((3, 4)));
        assert!(!throttle.should_report((3, 4)));
        assert!(throttle.should_report((4, 4)));
        throttle.reset();
        assert!(throttle.should_report((4, 4)));
    }

    #[test]
    fn click_counts_cycle_char_word_line() {
        assert_eq!(selection_mode_for(1, false), SelectionMode::Char);
        assert_eq!(selection_mode_for(2, false), SelectionMode::Word);
        assert_eq!(selection_mode_for(3, false), SelectionMode::Line);
        assert_eq!(selection_mode_for(4, false), SelectionMode::Char);
        assert_eq!(selection_mode_for(0, false), SelectionMode::Char);
    }

    #[test]
    fn alt_drag_selects_a_block() {
        assert_eq!(selection_mode_for(1, true), SelectionMode::Block);
    }

    #[test]
    fn the_scrollbar_is_hit_tested_before_the_grid() {
        let thumb = Some(ScrollbarThumb {
            y: 50.0,
            height: 40.0,
        });
        assert_eq!(hit_zone(100.0, 60.0, 390.0, true, thumb), HitZone::Cell);
        assert_eq!(
            hit_zone(395.0, 60.0, 390.0, true, thumb),
            HitZone::ScrollbarThumb
        );
        assert_eq!(
            hit_zone(395.0, 10.0, 390.0, true, thumb),
            HitZone::ScrollbarTrack
        );
        // Hidden scrollbar: the whole width is grid.
        assert_eq!(hit_zone(395.0, 60.0, 390.0, false, thumb), HitZone::Cell);
        // Visible but nothing to scroll: the track still swallows the click
        // rather than starting a selection in the padding.
        assert_eq!(
            hit_zone(395.0, 60.0, 390.0, true, None),
            HitZone::ScrollbarTrack
        );
    }

    #[test]
    fn only_the_three_real_buttons_are_reportable() {
        assert_eq!(
            button_from_gpui(gpui::MouseButton::Left),
            Some(MouseButton::Left)
        );
        assert_eq!(
            button_from_gpui(gpui::MouseButton::Middle),
            Some(MouseButton::Middle)
        );
        assert_eq!(
            button_from_gpui(gpui::MouseButton::Right),
            Some(MouseButton::Right)
        );
        assert_eq!(
            button_from_gpui(gpui::MouseButton::Navigate(gpui::NavigationDirection::Back)),
            None
        );
    }

    #[test]
    fn shift_hands_the_mouse_back_to_the_user() {
        use st_client_core::keys::Mods;
        use st_client_core::mouse::reports_to_program;
        use st_proto::Modes;
        assert!(reports_to_program(Modes::MOUSE_CLICK, Mods::empty()));
        assert!(!reports_to_program(Modes::MOUSE_CLICK, Mods::SHIFT));
        assert!(!reports_to_program(Modes::empty(), Mods::empty()));
    }
}
