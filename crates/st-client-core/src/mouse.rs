//! Mouse reporting and wheel handling — `docs/plan/04-client-native.md` §8.
//!
//! Four reporting protocols and two encodings, all pure functions over a
//! platform-neutral [`MouseEvent`]:
//!
//! | Mode | [`MouseProtocol`] | reports |
//! |---|---|---|
//! | 9 | [`X10`](MouseProtocol::X10) | press only, no modifiers |
//! | 1000 | [`Normal`](MouseProtocol::Normal) | press + release |
//! | 1002 | [`ButtonEvent`](MouseProtocol::ButtonEvent) | + motion while a button is down |
//! | 1003 | [`AnyEvent`](MouseProtocol::AnyEvent) | + motion with no button |
//!
//! The default (X10-style) encoding writes each coordinate as one byte,
//! `32 + n`, so it cannot express a column past **223**. Mode 1006 (SGR)
//! writes decimal parameters and has no such limit; it is what every modern
//! program enables, and the only correct choice on a wide window.
//!
//! ```
//! use st_client_core::mouse::{
//!     encode_mouse, MouseButton, MouseEncoding, MouseEvent, MouseEventKind, MouseProtocol,
//! };
//! use st_client_core::keys::Mods;
//!
//! let press = MouseEvent {
//!     kind: MouseEventKind::Press,
//!     button: MouseButton::Left,
//!     cell: (0, 0),
//!     mods: Mods::empty(),
//! };
//! assert_eq!(
//!     encode_mouse(&press, MouseProtocol::Normal, MouseEncoding::Sgr),
//!     Some(b"\x1b[<0;1;1M".to_vec())
//! );
//! ```

use st_proto::Modes;

use crate::keys::{Mods, ESC};

/// Which mouse-reporting protocol the program has enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MouseProtocol {
    /// No reporting: the client owns the mouse (selection, scrolling).
    #[default]
    Off,
    /// Mode 9: presses only, modifiers not reported.
    X10,
    /// Mode 1000: presses and releases.
    Normal,
    /// Mode 1002: presses, releases and motion while a button is held.
    ButtonEvent,
    /// Mode 1003: presses, releases and all motion.
    AnyEvent,
}

impl MouseProtocol {
    /// Reads the protocol out of the Surface's [`Modes`].
    ///
    /// The most capable enabled mode wins, which is how a terminal behaves
    /// when a program sets 1002 and then 1003.
    #[must_use]
    pub fn from_modes(modes: Modes) -> Self {
        if modes.contains(Modes::MOUSE_MOTION) {
            Self::AnyEvent
        } else if modes.contains(Modes::MOUSE_DRAG) {
            Self::ButtonEvent
        } else if modes.contains(Modes::MOUSE_CLICK) {
            Self::Normal
        } else {
            Self::Off
        }
    }

    /// `true` when the program wants mouse events at all.
    #[inline]
    #[must_use]
    pub const fn is_on(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// How a report is serialised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MouseEncoding {
    /// The original `ESC[M Cb Cx Cy` encoding: one byte per field, `32 + n`,
    /// so coordinates above [`X10_MAX_COORD`] cannot be expressed.
    #[default]
    Default,
    /// Mode 1006: `ESC[<b;x;yM` for press/motion, `…m` for release.
    Sgr,
}

impl MouseEncoding {
    /// Reads the encoding out of the Surface's [`Modes`].
    #[must_use]
    pub fn from_modes(modes: Modes) -> Self {
        if modes.contains(Modes::MOUSE_SGR) {
            Self::Sgr
        } else {
            Self::Default
        }
    }
}

/// The largest 1-based coordinate the default encoding can carry: `32 + 223`
/// is `255`, the last byte value.
pub const X10_MAX_COORD: u16 = 223;

/// Which button an event concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// Button 1.
    Left,
    /// Button 2.
    Middle,
    /// Button 3.
    Right,
    /// Wheel up (reported as button 64).
    WheelUp,
    /// Wheel down (reported as button 65).
    WheelDown,
    /// Wheel left (reported as button 66).
    WheelLeft,
    /// Wheel right (reported as button 67).
    WheelRight,
    /// No button — only meaningful for motion under
    /// [`MouseProtocol::AnyEvent`].
    None,
}

impl MouseButton {
    /// The button number xterm puts in the low bits of `Cb`.
    #[must_use]
    const fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
            Self::None => 3,
            Self::WheelUp => 64,
            Self::WheelDown => 65,
            Self::WheelLeft => 66,
            Self::WheelRight => 67,
        }
    }

    /// `true` for the four wheel pseudo-buttons.
    #[inline]
    #[must_use]
    pub const fn is_wheel(self) -> bool {
        matches!(
            self,
            Self::WheelUp | Self::WheelDown | Self::WheelLeft | Self::WheelRight
        )
    }
}

/// What happened to the button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseEventKind {
    /// A button went down, or the wheel turned one notch.
    Press,
    /// A button came up.
    Release,
    /// The pointer moved to another cell.
    Motion,
}

/// One mouse event, in grid coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MouseEvent {
    /// Press, release or motion.
    pub kind: MouseEventKind,
    /// Which button; [`MouseButton::None`] for button-less motion.
    pub button: MouseButton,
    /// `(col, row)`, both **0-based** within the visible grid. The encoders
    /// add the 1-based offset the wire format wants.
    pub cell: (u16, u16),
    /// Modifiers held at the time.
    pub mods: Mods,
}

impl MouseEvent {
    /// A press of `button` at `cell`.
    #[must_use]
    pub const fn press(button: MouseButton, cell: (u16, u16), mods: Mods) -> Self {
        Self {
            kind: MouseEventKind::Press,
            button,
            cell,
            mods,
        }
    }

    /// A release of `button` at `cell`.
    #[must_use]
    pub const fn release(button: MouseButton, cell: (u16, u16), mods: Mods) -> Self {
        Self {
            kind: MouseEventKind::Release,
            button,
            cell,
            mods,
        }
    }

    /// Motion to `cell`, with `button` held (or [`MouseButton::None`]).
    #[must_use]
    pub const fn motion(button: MouseButton, cell: (u16, u16), mods: Mods) -> Self {
        Self {
            kind: MouseEventKind::Motion,
            button,
            cell,
            mods,
        }
    }
}

/// The modifier bits xterm ORs into `Cb`.
fn modifier_bits(mods: Mods) -> u8 {
    let mut bits = 0;
    if mods.contains(Mods::SHIFT) {
        bits |= 4;
    }
    if mods.contains(Mods::ALT) {
        bits |= 8;
    }
    if mods.contains(Mods::CTRL) {
        bits |= 16;
    }
    bits
}

/// `true` when `protocol` reports this event at all.
#[must_use]
pub fn protocol_reports(event: &MouseEvent, protocol: MouseProtocol) -> bool {
    match protocol {
        MouseProtocol::Off => false,
        MouseProtocol::X10 => event.kind == MouseEventKind::Press && !event.button.is_wheel(),
        MouseProtocol::Normal => event.kind != MouseEventKind::Motion,
        MouseProtocol::ButtonEvent => {
            event.kind != MouseEventKind::Motion || event.button != MouseButton::None
        }
        MouseProtocol::AnyEvent => true,
    }
}

/// Encodes a mouse event as the bytes to write to the PTY, or `None` when the
/// event is not reported under `protocol` (or cannot be expressed — see
/// [`X10_MAX_COORD`]).
///
/// Wheel notches are always presses; a release of a wheel button is never
/// sent, matching xterm.
#[must_use]
pub fn encode_mouse(
    event: &MouseEvent,
    protocol: MouseProtocol,
    encoding: MouseEncoding,
) -> Option<Vec<u8>> {
    if !protocol_reports(event, protocol) {
        return None;
    }
    if event.button.is_wheel() && event.kind != MouseEventKind::Press {
        return None;
    }

    let (col, row) = event.cell;
    let x = col.checked_add(1)?;
    let y = row.checked_add(1)?;

    let mut cb = event.button.code();
    if protocol != MouseProtocol::X10 {
        cb |= modifier_bits(event.mods);
        if event.kind == MouseEventKind::Motion {
            cb |= 32;
        }
    }

    match encoding {
        MouseEncoding::Sgr => {
            let final_byte = if event.kind == MouseEventKind::Release {
                'm'
            } else {
                'M'
            };
            Some(format!("\x1b[<{cb};{x};{y}{final_byte}").into_bytes())
        }
        MouseEncoding::Default => {
            // The default encoding has no release button code: every release
            // reports button 3.
            if event.kind == MouseEventKind::Release {
                cb = 3 | modifier_bits(event.mods);
            }
            if x > X10_MAX_COORD || y > X10_MAX_COORD {
                // Unrepresentable. xterm silently drops these; so do we, and
                // the caller should have enabled SGR.
                return None;
            }
            Some(vec![ESC, b'[', b'M', 32 + cb, 32 + x as u8, 32 + y as u8])
        }
    }
}

// -------------------------------------------------------------------- wheel

/// What the alt screen does with the wheel when the program is *not* reading
/// the mouse (`config.altScreenScroll`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AltScreenScroll {
    /// Translate wheel notches into cursor-key presses, so `less`, `man` and
    /// `vim` scroll the way users expect. This is the default.
    #[default]
    Arrows,
    /// Do nothing.
    Off,
}

/// Wheel tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WheelConfig {
    /// Lines a discrete wheel notch scrolls.
    pub lines_per_notch: u16,
    /// What to do on the alt screen with no mouse reporting.
    pub alt_screen_scroll: AltScreenScroll,
}

impl Default for WheelConfig {
    fn default() -> Self {
        Self {
            lines_per_notch: 3,
            alt_screen_scroll: AltScreenScroll::Arrows,
        }
    }
}

/// What the caller should do with a wheel event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WheelAction {
    /// Move the local viewport by this many lines; positive scrolls **up**
    /// into history (i.e. increases the scroll offset).
    Scroll(i32),
    /// Write these bytes to the PTY: either a mouse report or the arrow-key
    /// emulation.
    Send(Vec<u8>),
    /// Nothing to do.
    None,
}

/// Decides what a wheel event means, per `04-client-native.md` §8:
///
/// * mouse reporting on (and Shift not held) → a mouse report for button 64/65;
/// * alt screen, no reporting, [`AltScreenScroll::Arrows`] → `n` cursor-key
///   presses, in the right form for [`Modes::APP_CURSOR_KEYS`];
/// * otherwise → scroll the local viewport.
///
/// `lines` is signed: positive scrolls **up** (towards older output), matching
/// [`WheelAction::Scroll`]. Trackpads pass a pixel-derived line count; discrete
/// wheels pass `±lines_per_notch`.
#[must_use]
pub fn handle_wheel(lines: i32, mods: Mods, modes: Modes, config: &WheelConfig) -> WheelAction {
    if lines == 0 {
        return WheelAction::None;
    }
    let protocol = MouseProtocol::from_modes(modes);
    let shift_override = mods.contains(Mods::SHIFT);

    if protocol.is_on() && !shift_override {
        // Reporting is the program's business; the caller supplies the cell.
        return WheelAction::None;
    }
    if modes.contains(Modes::ALT_SCREEN) {
        return match config.alt_screen_scroll {
            AltScreenScroll::Off => WheelAction::None,
            AltScreenScroll::Arrows => WheelAction::Send(wheel_to_arrows(lines, modes)),
        };
    }
    WheelAction::Scroll(lines)
}

/// Expands `lines` wheel lines into that many cursor-key presses: `ESC[A` /
/// `ESC[B`, or the SS3 form under [`Modes::APP_CURSOR_KEYS`].
///
/// Positive `lines` scrolls up, so it emits Up arrows.
#[must_use]
pub fn wheel_to_arrows(lines: i32, modes: Modes) -> Vec<u8> {
    let up = lines > 0;
    let count = lines.unsigned_abs() as usize;
    let seq: &[u8] = match (modes.contains(Modes::APP_CURSOR_KEYS), up) {
        (false, true) => b"\x1b[A",
        (false, false) => b"\x1b[B",
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1bOB",
    };
    let mut out = Vec::with_capacity(seq.len() * count);
    for _ in 0..count {
        out.extend_from_slice(seq);
    }
    out
}

/// The wheel pseudo-button for a signed line delta (positive = up).
#[must_use]
pub const fn wheel_button(lines: i32) -> MouseButton {
    if lines > 0 {
        MouseButton::WheelUp
    } else {
        MouseButton::WheelDown
    }
}

/// `true` when the pointer belongs to the program rather than to local
/// selection: reporting is on and Shift is not held (§8, "mouse mode
/// override").
#[must_use]
pub fn reports_to_program(modes: Modes, mods: Mods) -> bool {
    MouseProtocol::from_modes(modes).is_on() && !mods.contains(Mods::SHIFT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sgr(event: &MouseEvent, protocol: MouseProtocol) -> Option<String> {
        encode_mouse(event, protocol, MouseEncoding::Sgr)
            .map(|b| String::from_utf8(b).expect("ascii"))
    }

    #[test]
    fn sgr_press_release_and_drag() {
        let cell = (10, 4);
        assert_eq!(
            sgr(
                &MouseEvent::press(MouseButton::Left, cell, Mods::empty()),
                MouseProtocol::Normal
            )
            .as_deref(),
            Some("\x1b[<0;11;5M")
        );
        assert_eq!(
            sgr(
                &MouseEvent::release(MouseButton::Left, cell, Mods::empty()),
                MouseProtocol::Normal
            )
            .as_deref(),
            Some("\x1b[<0;11;5m")
        );
        assert_eq!(
            sgr(
                &MouseEvent::press(MouseButton::Middle, cell, Mods::empty()),
                MouseProtocol::Normal
            )
            .as_deref(),
            Some("\x1b[<1;11;5M")
        );
        assert_eq!(
            sgr(
                &MouseEvent::press(MouseButton::Right, cell, Mods::empty()),
                MouseProtocol::Normal
            )
            .as_deref(),
            Some("\x1b[<2;11;5M")
        );
        // Drag: button 0 + the 32 motion bit.
        assert_eq!(
            sgr(
                &MouseEvent::motion(MouseButton::Left, (3, 2), Mods::empty()),
                MouseProtocol::ButtonEvent
            )
            .as_deref(),
            Some("\x1b[<32;4;3M")
        );
        // Button-less motion: button 3 + 32 = 35, only under 1003.
        assert_eq!(
            sgr(
                &MouseEvent::motion(MouseButton::None, (3, 2), Mods::empty()),
                MouseProtocol::AnyEvent
            )
            .as_deref(),
            Some("\x1b[<35;4;3M")
        );
    }

    #[test]
    fn sgr_modifier_bits() {
        let at = |mods| {
            sgr(
                &MouseEvent::press(MouseButton::Left, (0, 0), mods),
                MouseProtocol::Normal,
            )
        };
        assert_eq!(at(Mods::SHIFT).as_deref(), Some("\x1b[<4;1;1M"));
        assert_eq!(at(Mods::ALT).as_deref(), Some("\x1b[<8;1;1M"));
        assert_eq!(at(Mods::CTRL).as_deref(), Some("\x1b[<16;1;1M"));
        assert_eq!(
            at(Mods::SHIFT | Mods::ALT | Mods::CTRL).as_deref(),
            Some("\x1b[<28;1;1M")
        );
        // Super is not a terminal modifier and is ignored.
        assert_eq!(at(Mods::SUPER).as_deref(), Some("\x1b[<0;1;1M"));
    }

    #[test]
    fn wheel_notches_are_buttons_64_and_65() {
        assert_eq!(
            sgr(
                &MouseEvent::press(MouseButton::WheelUp, (0, 0), Mods::empty()),
                MouseProtocol::Normal
            )
            .as_deref(),
            Some("\x1b[<64;1;1M")
        );
        assert_eq!(
            sgr(
                &MouseEvent::press(MouseButton::WheelDown, (0, 0), Mods::empty()),
                MouseProtocol::Normal
            )
            .as_deref(),
            Some("\x1b[<65;1;1M")
        );
        assert_eq!(
            sgr(
                &MouseEvent::press(MouseButton::WheelLeft, (0, 0), Mods::empty()),
                MouseProtocol::Normal
            )
            .as_deref(),
            Some("\x1b[<66;1;1M")
        );
        // A wheel "release" is never reported.
        assert_eq!(
            sgr(
                &MouseEvent::release(MouseButton::WheelUp, (0, 0), Mods::empty()),
                MouseProtocol::Normal
            ),
            None
        );
        // X10 does not know about the wheel at all.
        assert_eq!(
            encode_mouse(
                &MouseEvent::press(MouseButton::WheelUp, (0, 0), Mods::empty()),
                MouseProtocol::X10,
                MouseEncoding::Default
            ),
            None
        );
        assert_eq!(wheel_button(3), MouseButton::WheelUp);
        assert_eq!(wheel_button(-3), MouseButton::WheelDown);
        assert!(MouseButton::WheelDown.is_wheel());
        assert!(!MouseButton::Left.is_wheel());
    }

    #[test]
    fn default_encoding_byte_layout() {
        let bytes = encode_mouse(
            &MouseEvent::press(MouseButton::Left, (0, 0), Mods::empty()),
            MouseProtocol::Normal,
            MouseEncoding::Default,
        )
        .unwrap();
        assert_eq!(bytes, vec![ESC, b'[', b'M', 32, 33, 33]);

        let bytes = encode_mouse(
            &MouseEvent::press(MouseButton::Right, (9, 4), Mods::CTRL),
            MouseProtocol::Normal,
            MouseEncoding::Default,
        )
        .unwrap();
        assert_eq!(bytes, vec![ESC, b'[', b'M', 32 + 2 + 16, 32 + 10, 32 + 5]);

        // Every release is button 3 in the default encoding.
        let bytes = encode_mouse(
            &MouseEvent::release(MouseButton::Right, (0, 0), Mods::empty()),
            MouseProtocol::Normal,
            MouseEncoding::Default,
        )
        .unwrap();
        assert_eq!(bytes[3], 32 + 3);
    }

    #[test]
    fn x10_reports_presses_only_and_ignores_modifiers() {
        let press = MouseEvent::press(MouseButton::Middle, (1, 1), Mods::CTRL | Mods::SHIFT);
        assert_eq!(
            encode_mouse(&press, MouseProtocol::X10, MouseEncoding::Default).unwrap(),
            vec![ESC, b'[', b'M', 32 + 1, 34, 34]
        );
        assert_eq!(
            encode_mouse(
                &MouseEvent::release(MouseButton::Middle, (1, 1), Mods::empty()),
                MouseProtocol::X10,
                MouseEncoding::Default
            ),
            None
        );
        assert_eq!(
            encode_mouse(
                &MouseEvent::motion(MouseButton::Left, (1, 1), Mods::empty()),
                MouseProtocol::X10,
                MouseEncoding::Default
            ),
            None
        );
    }

    #[test]
    fn coordinates_past_223_need_sgr() {
        // 1-based column 223 is the last one the default encoding can carry.
        let ok = MouseEvent::press(MouseButton::Left, (222, 0), Mods::empty());
        let bytes = encode_mouse(&ok, MouseProtocol::Normal, MouseEncoding::Default).unwrap();
        assert_eq!(bytes[4], 255);

        let too_wide = MouseEvent::press(MouseButton::Left, (223, 0), Mods::empty());
        assert_eq!(
            encode_mouse(&too_wide, MouseProtocol::Normal, MouseEncoding::Default),
            None
        );
        let too_tall = MouseEvent::press(MouseButton::Left, (0, 400), Mods::empty());
        assert_eq!(
            encode_mouse(&too_tall, MouseProtocol::Normal, MouseEncoding::Default),
            None
        );

        // SGR has no such limit.
        assert_eq!(
            sgr(&too_wide, MouseProtocol::Normal).as_deref(),
            Some("\x1b[<0;224;1M")
        );
        assert_eq!(
            sgr(
                &MouseEvent::press(MouseButton::Left, (999, 1234), Mods::empty()),
                MouseProtocol::Normal
            )
            .as_deref(),
            Some("\x1b[<0;1000;1235M")
        );
    }

    #[test]
    fn motion_is_gated_on_the_protocol() {
        let drag = MouseEvent::motion(MouseButton::Left, (1, 1), Mods::empty());
        let hover = MouseEvent::motion(MouseButton::None, (1, 1), Mods::empty());
        for (proto, drag_ok, hover_ok) in [
            (MouseProtocol::Off, false, false),
            (MouseProtocol::X10, false, false),
            (MouseProtocol::Normal, false, false),
            (MouseProtocol::ButtonEvent, true, false),
            (MouseProtocol::AnyEvent, true, true),
        ] {
            assert_eq!(
                protocol_reports(&drag, proto),
                drag_ok,
                "drag under {proto:?}"
            );
            assert_eq!(
                protocol_reports(&hover, proto),
                hover_ok,
                "hover under {proto:?}"
            );
        }
        assert_eq!(sgr(&drag, MouseProtocol::Off), None);
    }

    #[test]
    fn protocol_and_encoding_come_from_the_modes() {
        assert_eq!(
            MouseProtocol::from_modes(Modes::empty()),
            MouseProtocol::Off
        );
        assert_eq!(
            MouseProtocol::from_modes(Modes::MOUSE_CLICK),
            MouseProtocol::Normal
        );
        assert_eq!(
            MouseProtocol::from_modes(Modes::MOUSE_CLICK | Modes::MOUSE_DRAG),
            MouseProtocol::ButtonEvent
        );
        assert_eq!(
            MouseProtocol::from_modes(Modes::MOUSE_MOTION | Modes::MOUSE_DRAG),
            MouseProtocol::AnyEvent
        );
        assert!(!MouseProtocol::Off.is_on());
        assert!(MouseProtocol::Normal.is_on());

        assert_eq!(
            MouseEncoding::from_modes(Modes::empty()),
            MouseEncoding::Default
        );
        assert_eq!(
            MouseEncoding::from_modes(Modes::MOUSE_SGR),
            MouseEncoding::Sgr
        );
    }

    #[test]
    fn shift_takes_the_mouse_back_for_local_selection() {
        assert!(reports_to_program(Modes::MOUSE_CLICK, Mods::empty()));
        assert!(!reports_to_program(Modes::MOUSE_CLICK, Mods::SHIFT));
        assert!(!reports_to_program(Modes::empty(), Mods::empty()));
    }

    #[test]
    fn wheel_scrolls_the_viewport_on_the_primary_screen() {
        let cfg = WheelConfig::default();
        assert_eq!(
            handle_wheel(3, Mods::empty(), Modes::empty(), &cfg),
            WheelAction::Scroll(3)
        );
        assert_eq!(
            handle_wheel(-3, Mods::empty(), Modes::empty(), &cfg),
            WheelAction::Scroll(-3)
        );
        assert_eq!(
            handle_wheel(0, Mods::empty(), Modes::empty(), &cfg),
            WheelAction::None
        );
        assert_eq!(cfg.lines_per_notch, 3);
    }

    #[test]
    fn wheel_becomes_arrow_keys_on_the_alt_screen() {
        let cfg = WheelConfig::default();
        assert_eq!(
            handle_wheel(2, Mods::empty(), Modes::ALT_SCREEN, &cfg),
            WheelAction::Send(b"\x1b[A\x1b[A".to_vec())
        );
        assert_eq!(
            handle_wheel(-3, Mods::empty(), Modes::ALT_SCREEN, &cfg),
            WheelAction::Send(b"\x1b[B\x1b[B\x1b[B".to_vec())
        );
        assert_eq!(
            handle_wheel(
                1,
                Mods::empty(),
                Modes::ALT_SCREEN | Modes::APP_CURSOR_KEYS,
                &cfg
            ),
            WheelAction::Send(b"\x1bOA".to_vec())
        );

        let off = WheelConfig {
            alt_screen_scroll: AltScreenScroll::Off,
            ..cfg
        };
        assert_eq!(
            handle_wheel(2, Mods::empty(), Modes::ALT_SCREEN, &off),
            WheelAction::None
        );
        assert_eq!(wheel_to_arrows(0, Modes::empty()), Vec::<u8>::new());
    }

    #[test]
    fn wheel_defers_to_the_program_when_reporting_is_on() {
        let cfg = WheelConfig::default();
        // The caller must encode a wheel button report instead.
        assert_eq!(
            handle_wheel(3, Mods::empty(), Modes::MOUSE_CLICK, &cfg),
            WheelAction::None
        );
        // Shift wrests it back for local scrolling.
        assert_eq!(
            handle_wheel(3, Mods::SHIFT, Modes::MOUSE_CLICK, &cfg),
            WheelAction::Scroll(3)
        );
        // Shift on the alt screen with reporting still uses arrow emulation.
        assert_eq!(
            handle_wheel(1, Mods::SHIFT, Modes::MOUSE_CLICK | Modes::ALT_SCREEN, &cfg),
            WheelAction::Send(b"\x1b[A".to_vec())
        );
    }
}
