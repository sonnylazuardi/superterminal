//! xterm-compatible key encoding — `docs/plan/04-client-native.md` §7.
//!
//! The Client never interprets VT bytes (invariant I1) but it *produces* them:
//! a keystroke becomes the exact byte sequence xterm would send, and the
//! Server writes it to the PTY verbatim (`02-protocol.md` §9).
//!
//! The input type is [`KeyEvent`], which is deliberately platform-neutral —
//! this crate has no GPUI dependency (invariant I9), so the GPUI layer maps
//! `gpui::Keystroke` onto [`Key`] and [`Mods`] and everything below is a pure
//! function with table-driven tests.
//!
//! ```
//! use st_client_core::keys::{encode_key, Key, KeyConfig, KeyEvent, Mods};
//! use st_proto::Modes;
//!
//! let ctrl_c = KeyEvent::new(Key::Char('c'), Mods::CTRL);
//! assert_eq!(encode_key(&ctrl_c, Modes::empty(), &KeyConfig::default()), Some(vec![0x03]));
//!
//! let up = KeyEvent::plain(Key::Up);
//! assert_eq!(encode_key(&up, Modes::empty(), &KeyConfig::default()), Some(b"\x1b[A".to_vec()));
//! assert_eq!(
//!     encode_key(&up, Modes::APP_CURSOR_KEYS, &KeyConfig::default()),
//!     Some(b"\x1bOA".to_vec())
//! );
//! ```

use st_proto::Modes;

/// The escape byte, `0x1B`.
pub const ESC: u8 = 0x1B;

/// Start of a bracketed paste (mode 2004).
pub const PASTE_START: &[u8] = b"\x1b[200~";

/// End of a bracketed paste (mode 2004).
pub const PASTE_END: &[u8] = b"\x1b[201~";

bitflags::bitflags! {
    /// Keyboard modifiers, in the order xterm's modifier parameter expects.
    ///
    /// [`Mods::SUPER`] (Command on macOS, Super on Linux) never reaches the
    /// PTY: xterm has no encoding for it, so [`encode_key`] returns `None` and
    /// the chord bubbles up to the app's command registry (§7, grilling Q23).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct Mods: u8 {
        /// Shift.
        const SHIFT = 1 << 0;
        /// Alt / Option / Meta.
        const ALT = 1 << 1;
        /// Control.
        const CTRL = 1 << 2;
        /// Command / Super / Windows.
        const SUPER = 1 << 3;
    }
}

impl Mods {
    /// xterm's modifier parameter: `1 + shift·1 + alt·2 + ctrl·4` (§7).
    ///
    /// `1` means "no modifiers", which is the value xterm omits from a
    /// sequence entirely.
    #[inline]
    #[must_use]
    pub const fn xterm_param(self) -> u8 {
        1 + (self.bits() & 0b111)
    }

    /// `true` when no modifier that changes the encoding is held.
    #[inline]
    #[must_use]
    pub const fn is_plain(self) -> bool {
        self.xterm_param() == 1
    }
}

/// A key on the keyboard, independent of any UI toolkit.
///
/// `Char` carries the character the layout produced *after* shift is applied
/// (`Shift+a` is `Char('A')`), which is what every platform hands us and what
/// xterm encodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// A character-producing key.
    Char(char),
    /// Return / Enter on the main block.
    Enter,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
    /// Cursor up.
    Up,
    /// Cursor down.
    Down,
    /// Cursor right.
    Right,
    /// Cursor left.
    Left,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Insert.
    Insert,
    /// Forward delete.
    Delete,
    /// A function key; `1..=12` are encoded, anything else yields `None`.
    Function(u8),
    /// A key on the numeric keypad.
    Keypad(Keypad),
}

/// Numeric-keypad keys, which change encoding under DECPAM
/// ([`Modes::APP_KEYPAD`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keypad {
    /// A digit, `0..=9`.
    Digit(u8),
    /// `KP_Enter`.
    Enter,
    /// `KP_Add`.
    Plus,
    /// `KP_Subtract`.
    Minus,
    /// `KP_Multiply`.
    Multiply,
    /// `KP_Divide`.
    Divide,
    /// `KP_Decimal`.
    Decimal,
    /// `KP_Equal`.
    Equal,
}

/// One keystroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    /// Which key.
    pub key: Key,
    /// Which modifiers were held.
    pub mods: Mods,
}

impl KeyEvent {
    /// A keystroke with modifiers.
    #[inline]
    #[must_use]
    pub const fn new(key: Key, mods: Mods) -> Self {
        Self { key, mods }
    }

    /// A keystroke with no modifiers.
    #[inline]
    #[must_use]
    pub const fn plain(key: Key) -> Self {
        Self {
            key,
            mods: Mods::empty(),
        }
    }
}

/// Client-side keyboard options (`config.toml` `[keys]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyConfig {
    /// Backspace sends `DEL` (`0x7F`) rather than `BS` (`0x08`). This is the
    /// modern default; `Ctrl+Backspace` sends the other one, as in xterm.
    pub backspace_sends_del: bool,
    /// Alt/Option prefixes the key with `ESC`. On macOS this is off by default
    /// in the app config so Option can compose characters; the encoder itself
    /// defaults to `true` because that is xterm's behaviour.
    pub alt_sends_esc: bool,
    /// Keep the trailing newline of a paste (xterm behaviour, §9).
    pub paste_keeps_trailing_newline: bool,
}

impl Default for KeyConfig {
    fn default() -> Self {
        Self {
            backspace_sends_del: true,
            alt_sends_esc: true,
            paste_keeps_trailing_newline: true,
        }
    }
}

/// Encodes a keystroke as the bytes to write to the PTY.
///
/// `None` means "not a terminal key": the caller must let the event propagate
/// so the app's command registry can claim it (§7). That covers every chord
/// holding [`Mods::SUPER`], unsupported function keys, and control characters
/// with no xterm encoding.
///
/// The `modes` that matter here are [`Modes::APP_CURSOR_KEYS`] (DECCKM) and
/// [`Modes::APP_KEYPAD`] (DECPAM).
#[must_use]
pub fn encode_key(event: &KeyEvent, modes: Modes, config: &KeyConfig) -> Option<Vec<u8>> {
    if event.mods.contains(Mods::SUPER) {
        return None;
    }
    let mods = event.mods;
    let app_cursor = modes.contains(Modes::APP_CURSOR_KEYS);

    let bytes = match event.key {
        Key::Char(ch) => return encode_char(ch, mods, config),

        Key::Enter => alt_prefixed(b"\r".to_vec(), mods, config),
        Key::Tab if mods.contains(Mods::SHIFT) => b"\x1b[Z".to_vec(),
        Key::Tab => alt_prefixed(b"\t".to_vec(), mods, config),
        Key::Escape => alt_prefixed(vec![ESC], mods, config),
        Key::Backspace => {
            // xterm: Ctrl inverts the DEL/BS choice.
            let del = config.backspace_sends_del != mods.contains(Mods::CTRL);
            alt_prefixed(vec![if del { 0x7F } else { 0x08 }], mods, config)
        }

        Key::Up => cursor_key(b'A', mods, app_cursor),
        Key::Down => cursor_key(b'B', mods, app_cursor),
        Key::Right => cursor_key(b'C', mods, app_cursor),
        Key::Left => cursor_key(b'D', mods, app_cursor),
        Key::Home => cursor_key(b'H', mods, app_cursor),
        Key::End => cursor_key(b'F', mods, app_cursor),

        Key::Insert => tilde_key(2, mods),
        Key::Delete => tilde_key(3, mods),
        Key::PageUp => tilde_key(5, mods),
        Key::PageDown => tilde_key(6, mods),

        // F1–F4 are SS3 sequences; modified they become CSI 1;m P..S.
        Key::Function(n @ 1..=4) => {
            let final_byte = b'P' + (n - 1);
            if mods.is_plain() {
                vec![ESC, b'O', final_byte]
            } else {
                let mut out = format!("\x1b[1;{}", mods.xterm_param()).into_bytes();
                out.push(final_byte);
                out
            }
        }
        Key::Function(n @ 5..=12) => tilde_key(FUNCTION_TILDE[(n - 5) as usize], mods),
        Key::Function(_) => return None,

        Key::Keypad(kp) => return encode_keypad(kp, mods, modes, config),
    };
    Some(bytes)
}

/// `~`-terminated CSI numbers for F5..F12 (§7).
const FUNCTION_TILDE: [u8; 8] = [15, 17, 18, 19, 20, 21, 23, 24];

/// `ESC[{code}~`, or `ESC[{code};{m}~` when modified.
fn tilde_key(code: u8, mods: Mods) -> Vec<u8> {
    if mods.is_plain() {
        format!("\x1b[{code}~").into_bytes()
    } else {
        format!("\x1b[{code};{}~", mods.xterm_param()).into_bytes()
    }
}

/// Arrows, Home and End: `ESC[X` normally, `ESC OX` under DECCKM, and always
/// the CSI form with an explicit modifier parameter when modified.
fn cursor_key(final_byte: u8, mods: Mods, app_cursor: bool) -> Vec<u8> {
    if !mods.is_plain() {
        let mut out = format!("\x1b[1;{}", mods.xterm_param()).into_bytes();
        out.push(final_byte);
        return out;
    }
    if app_cursor {
        vec![ESC, b'O', final_byte]
    } else {
        vec![ESC, b'[', final_byte]
    }
}

/// Prefixes `bytes` with `ESC` when Alt is held and configured to do so.
fn alt_prefixed(bytes: Vec<u8>, mods: Mods, config: &KeyConfig) -> Vec<u8> {
    if mods.contains(Mods::ALT) && config.alt_sends_esc {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        out.push(ESC);
        out.extend_from_slice(&bytes);
        out
    } else {
        bytes
    }
}

/// Encodes a character-producing key, applying the Ctrl table first.
fn encode_char(ch: char, mods: Mods, config: &KeyConfig) -> Option<Vec<u8>> {
    let base = if mods.contains(Mods::CTRL) {
        vec![control_byte(ch)?]
    } else {
        let mut buf = [0u8; 4];
        ch.encode_utf8(&mut buf).as_bytes().to_vec()
    };
    Some(alt_prefixed(base, mods, config))
}

/// The C0 control byte a `Ctrl+<ch>` chord produces, or `None` when xterm has
/// no encoding for it (§7).
#[must_use]
pub fn control_byte(ch: char) -> Option<u8> {
    let byte = match ch.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        ' ' | '@' | '2' => 0x00,
        '[' => 0x1B,
        '\\' | '4' => 0x1C,
        ']' | '5' => 0x1D,
        '^' | '6' => 0x1E,
        '_' | '7' | '/' => 0x1F,
        '3' => 0x1B,
        '8' | '?' => 0x7F,
        _ => return None,
    };
    Some(byte)
}

/// Keypad keys: literal characters normally, SS3 sequences under DECPAM.
fn encode_keypad(kp: Keypad, mods: Mods, modes: Modes, config: &KeyConfig) -> Option<Vec<u8>> {
    if modes.contains(Modes::APP_KEYPAD) && mods.is_plain() {
        let final_byte = match kp {
            Keypad::Digit(d @ 0..=9) => b'p' + d,
            Keypad::Digit(_) => return None,
            Keypad::Enter => b'M',
            Keypad::Plus => b'k',
            Keypad::Minus => b'm',
            Keypad::Multiply => b'j',
            Keypad::Divide => b'o',
            Keypad::Decimal => b'n',
            Keypad::Equal => b'X',
        };
        return Some(vec![ESC, b'O', final_byte]);
    }
    let ch = match kp {
        Keypad::Digit(d @ 0..=9) => (b'0' + d) as char,
        Keypad::Digit(_) => return None,
        Keypad::Enter => return Some(alt_prefixed(b"\r".to_vec(), mods, config)),
        Keypad::Plus => '+',
        Keypad::Minus => '-',
        Keypad::Multiply => '*',
        Keypad::Divide => '/',
        Keypad::Decimal => '.',
        Keypad::Equal => '=',
    };
    encode_char(ch, mods, config)
}

/// Encodes committed text (IME, a dead-key composition, a shifted glyph) as
/// UTF-8 PTY bytes.
#[must_use]
pub fn encode_text(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

// ------------------------------------------------------------------- paste

/// Prepares clipboard text for the PTY (`04-client-native.md` §9).
///
/// * `\r\n` and `\n` become `\r`, because that is what Enter sends and what
///   line-editing programs expect.
/// * Any embedded `ESC[201~` is stripped: otherwise pasted text could end the
///   bracket early and the rest would be read as keystrokes (paste-injection
///   guard).
/// * The whole thing is wrapped in `ESC[200~ … ESC[201~` when the program has
///   enabled bracketed paste ([`Modes::BRACKETED_PASTE`], mode 2004).
#[must_use]
pub fn prepare_paste(text: &str, modes: Modes, config: &KeyConfig) -> Vec<u8> {
    let mut body = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                body.push('\r');
            }
            '\n' => body.push('\r'),
            other => body.push(other),
        }
    }
    if !config.paste_keeps_trailing_newline {
        while body.ends_with('\r') {
            body.pop();
        }
    }
    let body = strip_paste_end(&body);

    if modes.contains(Modes::BRACKETED_PASTE) {
        let mut out = Vec::with_capacity(body.len() + PASTE_START.len() + PASTE_END.len());
        out.extend_from_slice(PASTE_START);
        out.extend_from_slice(body.as_bytes());
        out.extend_from_slice(PASTE_END);
        out
    } else {
        body.into_bytes()
    }
}

/// Wraps already-prepared bytes in the bracketed-paste markers unconditionally.
///
/// Useful when the caller has its own normalisation; [`prepare_paste`] is the
/// one to reach for otherwise.
#[must_use]
pub fn bracket_paste(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + PASTE_START.len() + PASTE_END.len());
    out.extend_from_slice(PASTE_START);
    out.extend_from_slice(body);
    out.extend_from_slice(PASTE_END);
    out
}

/// Removes every occurrence of the paste-end marker from `text`.
fn strip_paste_end(text: &str) -> String {
    const END: &str = "\x1b[201~";
    if !text.contains(END) {
        return text.to_string();
    }
    text.replace(END, "")
}

/// `true` when a paste needs the "are you sure?" confirmation: more than one
/// line and no bracketed-paste protection (§9, `confirmMultilinePaste`).
#[must_use]
pub fn paste_needs_confirmation(text: &str, modes: Modes) -> bool {
    !modes.contains(Modes::BRACKETED_PASTE) && text.trim_end_matches(['\r', '\n']).contains('\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(key: Key, mods: Mods, modes: Modes) -> Option<Vec<u8>> {
        encode_key(&KeyEvent::new(key, mods), modes, &KeyConfig::default())
    }

    fn s(key: Key, mods: Mods, modes: Modes) -> String {
        String::from_utf8(enc(key, mods, modes).expect("encodable")).expect("utf-8")
    }

    #[test]
    fn printable_characters_are_their_utf8() {
        assert_eq!(
            enc(Key::Char('a'), Mods::empty(), Modes::empty()),
            Some(b"a".to_vec())
        );
        assert_eq!(
            enc(Key::Char('A'), Mods::SHIFT, Modes::empty()),
            Some(b"A".to_vec())
        );
        assert_eq!(
            enc(Key::Char(' '), Mods::empty(), Modes::empty()),
            Some(b" ".to_vec())
        );
        assert_eq!(
            enc(Key::Char('é'), Mods::empty(), Modes::empty()),
            Some("é".as_bytes().to_vec())
        );
        assert_eq!(
            enc(Key::Char('世'), Mods::empty(), Modes::empty()),
            Some("世".as_bytes().to_vec())
        );
        assert_eq!(encode_text("héllo"), "héllo".as_bytes().to_vec());
    }

    #[test]
    fn ctrl_letters_are_c0_controls() {
        let table = [
            ('a', 0x01u8),
            ('c', 0x03),
            ('d', 0x04),
            ('i', 0x09),
            ('j', 0x0A),
            ('m', 0x0D),
            ('z', 0x1A),
        ];
        for (ch, byte) in table {
            assert_eq!(
                enc(Key::Char(ch), Mods::CTRL, Modes::empty()),
                Some(vec![byte]),
                "ctrl-{ch}"
            );
            // Case-insensitive: Ctrl+Shift+C is still 0x03.
            assert_eq!(
                enc(
                    Key::Char(ch.to_ascii_uppercase()),
                    Mods::CTRL | Mods::SHIFT,
                    Modes::empty()
                ),
                Some(vec![byte])
            );
        }
    }

    #[test]
    fn ctrl_punctuation_and_digits() {
        let table = [
            (' ', 0x00u8),
            ('@', 0x00),
            ('[', 0x1B),
            ('\\', 0x1C),
            (']', 0x1D),
            ('^', 0x1E),
            ('_', 0x1F),
            ('?', 0x7F),
            ('2', 0x00),
            ('3', 0x1B),
            ('4', 0x1C),
            ('5', 0x1D),
            ('6', 0x1E),
            ('7', 0x1F),
            ('8', 0x7F),
        ];
        for (ch, byte) in table {
            assert_eq!(
                enc(Key::Char(ch), Mods::CTRL, Modes::empty()),
                Some(vec![byte]),
                "ctrl-{ch}"
            );
        }
        // No xterm encoding: let the app have it.
        assert_eq!(enc(Key::Char('%'), Mods::CTRL, Modes::empty()), None);
        assert_eq!(control_byte('%'), None);
    }

    #[test]
    fn alt_prefixes_with_escape() {
        assert_eq!(
            enc(Key::Char('x'), Mods::ALT, Modes::empty()),
            Some(b"\x1bx".to_vec())
        );
        assert_eq!(
            enc(Key::Char('c'), Mods::ALT | Mods::CTRL, Modes::empty()),
            Some(vec![ESC, 0x03])
        );
        assert_eq!(
            enc(Key::Enter, Mods::ALT, Modes::empty()),
            Some(b"\x1b\r".to_vec())
        );
        assert_eq!(
            enc(Key::Escape, Mods::ALT, Modes::empty()),
            Some(vec![ESC, ESC])
        );

        let no_alt = KeyConfig {
            alt_sends_esc: false,
            ..KeyConfig::default()
        };
        assert_eq!(
            encode_key(
                &KeyEvent::new(Key::Char('x'), Mods::ALT),
                Modes::empty(),
                &no_alt
            ),
            Some(b"x".to_vec())
        );
    }

    #[test]
    fn super_chords_are_not_terminal_keys() {
        assert_eq!(enc(Key::Char('t'), Mods::SUPER, Modes::empty()), None);
        assert_eq!(
            enc(Key::Char('k'), Mods::SUPER | Mods::SHIFT, Modes::empty()),
            None
        );
        assert_eq!(enc(Key::Left, Mods::SUPER, Modes::empty()), None);
    }

    #[test]
    fn arrows_in_both_cursor_key_modes() {
        for (key, letter) in [
            (Key::Up, 'A'),
            (Key::Down, 'B'),
            (Key::Right, 'C'),
            (Key::Left, 'D'),
        ] {
            assert_eq!(
                s(key, Mods::empty(), Modes::empty()),
                format!("\x1b[{letter}")
            );
            assert_eq!(
                s(key, Mods::empty(), Modes::APP_CURSOR_KEYS),
                format!("\x1bO{letter}")
            );
            // Modified always uses the CSI form, in either mode.
            assert_eq!(
                s(key, Mods::CTRL, Modes::APP_CURSOR_KEYS),
                format!("\x1b[1;5{letter}")
            );
        }
    }

    #[test]
    fn modifier_parameters_follow_the_xterm_formula() {
        assert_eq!(Mods::empty().xterm_param(), 1);
        assert_eq!(Mods::SHIFT.xterm_param(), 2);
        assert_eq!(Mods::ALT.xterm_param(), 3);
        assert_eq!((Mods::SHIFT | Mods::ALT).xterm_param(), 4);
        assert_eq!(Mods::CTRL.xterm_param(), 5);
        assert_eq!((Mods::SHIFT | Mods::CTRL).xterm_param(), 6);
        assert_eq!((Mods::ALT | Mods::CTRL).xterm_param(), 7);
        assert_eq!((Mods::SHIFT | Mods::ALT | Mods::CTRL).xterm_param(), 8);
        // Super does not participate in the parameter.
        assert_eq!((Mods::SHIFT | Mods::SUPER).xterm_param(), 2);

        assert_eq!(s(Key::Up, Mods::SHIFT, Modes::empty()), "\x1b[1;2A");
        assert_eq!(
            s(
                Key::Right,
                Mods::SHIFT | Mods::ALT | Mods::CTRL,
                Modes::empty()
            ),
            "\x1b[1;8C"
        );
    }

    #[test]
    fn home_end_page_insert_delete() {
        assert_eq!(s(Key::Home, Mods::empty(), Modes::empty()), "\x1b[H");
        assert_eq!(s(Key::End, Mods::empty(), Modes::empty()), "\x1b[F");
        assert_eq!(
            s(Key::Home, Mods::empty(), Modes::APP_CURSOR_KEYS),
            "\x1bOH"
        );
        assert_eq!(s(Key::End, Mods::empty(), Modes::APP_CURSOR_KEYS), "\x1bOF");
        assert_eq!(s(Key::Home, Mods::CTRL, Modes::empty()), "\x1b[1;5H");

        assert_eq!(s(Key::Insert, Mods::empty(), Modes::empty()), "\x1b[2~");
        assert_eq!(s(Key::Delete, Mods::empty(), Modes::empty()), "\x1b[3~");
        assert_eq!(s(Key::PageUp, Mods::empty(), Modes::empty()), "\x1b[5~");
        assert_eq!(s(Key::PageDown, Mods::empty(), Modes::empty()), "\x1b[6~");
        assert_eq!(s(Key::Delete, Mods::SHIFT, Modes::empty()), "\x1b[3;2~");
        assert_eq!(s(Key::PageUp, Mods::CTRL, Modes::empty()), "\x1b[5;5~");
    }

    #[test]
    fn function_keys_f1_to_f12() {
        let table = [
            (1u8, "\x1bOP"),
            (2, "\x1bOQ"),
            (3, "\x1bOR"),
            (4, "\x1bOS"),
            (5, "\x1b[15~"),
            (6, "\x1b[17~"),
            (7, "\x1b[18~"),
            (8, "\x1b[19~"),
            (9, "\x1b[20~"),
            (10, "\x1b[21~"),
            (11, "\x1b[23~"),
            (12, "\x1b[24~"),
        ];
        for (n, expected) in table {
            assert_eq!(
                s(Key::Function(n), Mods::empty(), Modes::empty()),
                expected,
                "F{n}"
            );
        }
        assert_eq!(
            s(Key::Function(1), Mods::SHIFT, Modes::empty()),
            "\x1b[1;2P"
        );
        assert_eq!(
            s(Key::Function(5), Mods::CTRL, Modes::empty()),
            "\x1b[15;5~"
        );
        assert_eq!(enc(Key::Function(13), Mods::empty(), Modes::empty()), None);
        assert_eq!(enc(Key::Function(0), Mods::empty(), Modes::empty()), None);
    }

    #[test]
    fn enter_tab_backtab_escape() {
        assert_eq!(
            enc(Key::Enter, Mods::empty(), Modes::empty()),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            enc(Key::Tab, Mods::empty(), Modes::empty()),
            Some(b"\t".to_vec())
        );
        assert_eq!(
            enc(Key::Tab, Mods::SHIFT, Modes::empty()),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            enc(Key::Escape, Mods::empty(), Modes::empty()),
            Some(vec![ESC])
        );
    }

    #[test]
    fn backspace_del_versus_bs_is_configurable() {
        let del = KeyConfig::default();
        let bs = KeyConfig {
            backspace_sends_del: false,
            ..KeyConfig::default()
        };
        let ev = |mods| KeyEvent::new(Key::Backspace, mods);

        assert_eq!(
            encode_key(&ev(Mods::empty()), Modes::empty(), &del),
            Some(vec![0x7F])
        );
        assert_eq!(
            encode_key(&ev(Mods::empty()), Modes::empty(), &bs),
            Some(vec![0x08])
        );
        // Ctrl inverts the choice, as in xterm.
        assert_eq!(
            encode_key(&ev(Mods::CTRL), Modes::empty(), &del),
            Some(vec![0x08])
        );
        assert_eq!(
            encode_key(&ev(Mods::CTRL), Modes::empty(), &bs),
            Some(vec![0x7F])
        );
        assert_eq!(
            encode_key(&ev(Mods::ALT), Modes::empty(), &del),
            Some(vec![ESC, 0x7F])
        );
    }

    #[test]
    fn keypad_normal_and_application_mode() {
        let cfg = KeyConfig::default();
        let k = |kp| KeyEvent::plain(Key::Keypad(kp));

        assert_eq!(
            encode_key(&k(Keypad::Digit(7)), Modes::empty(), &cfg),
            Some(b"7".to_vec())
        );
        assert_eq!(
            encode_key(&k(Keypad::Digit(0)), Modes::APP_KEYPAD, &cfg),
            Some(b"\x1bOp".to_vec())
        );
        assert_eq!(
            encode_key(&k(Keypad::Digit(9)), Modes::APP_KEYPAD, &cfg),
            Some(b"\x1bOy".to_vec())
        );
        assert_eq!(
            encode_key(&k(Keypad::Enter), Modes::APP_KEYPAD, &cfg),
            Some(b"\x1bOM".to_vec())
        );
        assert_eq!(
            encode_key(&k(Keypad::Enter), Modes::empty(), &cfg),
            Some(b"\r".to_vec())
        );
        for (kp, app, plain) in [
            (Keypad::Plus, "\x1bOk", "+"),
            (Keypad::Minus, "\x1bOm", "-"),
            (Keypad::Multiply, "\x1bOj", "*"),
            (Keypad::Divide, "\x1bOo", "/"),
            (Keypad::Decimal, "\x1bOn", "."),
            (Keypad::Equal, "\x1bOX", "="),
        ] {
            assert_eq!(
                encode_key(&k(kp), Modes::APP_KEYPAD, &cfg),
                Some(app.as_bytes().to_vec())
            );
            assert_eq!(
                encode_key(&k(kp), Modes::empty(), &cfg),
                Some(plain.as_bytes().to_vec())
            );
        }
        // A modified keypad key falls back to the literal encoding.
        assert_eq!(
            encode_key(
                &KeyEvent::new(Key::Keypad(Keypad::Digit(3)), Mods::CTRL),
                Modes::APP_KEYPAD,
                &cfg
            ),
            Some(vec![0x1B])
        );
        assert_eq!(
            encode_key(&k(Keypad::Digit(12)), Modes::empty(), &cfg),
            None
        );
    }

    #[test]
    fn paste_normalises_newlines_and_brackets() {
        let cfg = KeyConfig::default();
        assert_eq!(
            prepare_paste("a\nb\r\nc", Modes::empty(), &cfg),
            b"a\rb\rc".to_vec()
        );
        assert_eq!(
            prepare_paste("a\nb", Modes::BRACKETED_PASTE, &cfg),
            b"\x1b[200~a\rb\x1b[201~".to_vec()
        );
        assert_eq!(prepare_paste("x\n", Modes::empty(), &cfg), b"x\r".to_vec());

        let strip = KeyConfig {
            paste_keeps_trailing_newline: false,
            ..cfg
        };
        assert_eq!(
            prepare_paste("x\n\n", Modes::empty(), &strip),
            b"x".to_vec()
        );
    }

    #[test]
    fn paste_injection_guard_strips_the_end_marker() {
        let cfg = KeyConfig::default();
        let hostile = "safe\x1b[201~rm -rf /\r";
        let out = prepare_paste(hostile, Modes::BRACKETED_PASTE, &cfg);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "\x1b[200~saferm -rf /\r\x1b[201~");
        assert_eq!(text.matches("\x1b[201~").count(), 1);

        assert_eq!(bracket_paste(b"hi"), b"\x1b[200~hi\x1b[201~".to_vec());
    }

    #[test]
    fn multiline_paste_confirmation_rule() {
        assert!(paste_needs_confirmation("a\nb", Modes::empty()));
        assert!(!paste_needs_confirmation("a\n", Modes::empty()));
        assert!(!paste_needs_confirmation("a", Modes::empty()));
        assert!(!paste_needs_confirmation("a\nb", Modes::BRACKETED_PASTE));
    }
}
