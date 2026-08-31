//! gpui keystrokes → PTY bytes (04 §7).
//!
//! The table itself lives in `st_client_core::keys`; this module is only the
//! translation from gpui's naming (`"pageup"`, `"escape"`, a `key_char` that
//! already has the shift and the dead-key composition applied) into
//! [`st_client_core::KeyEvent`], plus the passthrough check that decides
//! whether the element consumes the event at all.

use st_client_core::keys::{encode_key, encode_text, Key, KeyConfig, KeyEvent, Mods, ESC};
use st_proto::Modes;

use crate::props::PassthroughKeys;

/// What the element does with a key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    /// Write these bytes to the PTY and stop propagation.
    Send(Vec<u8>),
    /// React claimed this chord in `passthroughKeys`: do not consume it, and
    /// emit a `shortcut` event so an app that cannot see the bubbled GPUI
    /// event still gets it (04 §7, HANDOVER V5).
    Passthrough,
    /// Not a terminal key and not claimed: do not consume it either.
    Ignore,
}

/// Decides what one keystroke means.
///
/// `key` is `gpui::Keystroke::key` (already lower-cased by gpui on every
/// platform) and `key_char` is `gpui::Keystroke::key_char`, which carries the
/// character the layout actually produces — `"ß"` for `alt-s`, `"!"` for
/// `shift-1`, the composed glyph for a dead key.
#[must_use]
pub fn handle_key(
    key: &str,
    key_char: Option<&str>,
    mods: Mods,
    passthrough: &PassthroughKeys,
    modes: Modes,
    config: &KeyConfig,
) -> KeyOutcome {
    if passthrough.contains(mods, key) {
        return KeyOutcome::Passthrough;
    }

    if let Some(named) = named_key(key) {
        return match encode_key(&KeyEvent::new(named, mods), modes, config) {
            Some(bytes) => KeyOutcome::Send(bytes),
            None => KeyOutcome::Ignore,
        };
    }

    // A multi-character `key_char` is an IME commit or a composed sequence:
    // there is no single `Key::Char` for it, so the text goes as-is.
    if let Some(text) = committed_text(key_char, mods) {
        if text.chars().count() > 1 {
            let mut bytes = Vec::new();
            if mods.contains(Mods::ALT) && config.alt_sends_esc {
                bytes.push(ESC);
            }
            bytes.extend_from_slice(&encode_text(&text));
            return KeyOutcome::Send(bytes);
        }
    }

    let Some(ch) = character(key, key_char, mods) else {
        return KeyOutcome::Ignore;
    };
    match encode_key(&KeyEvent::new(Key::Char(ch), mods), modes, config) {
        Some(bytes) => KeyOutcome::Send(bytes),
        None => KeyOutcome::Ignore,
    }
}

/// gpui's name for a non-character key → the `st-client-core` enum.
///
/// The names come from `gpui_linux`'s `keystroke_from_xkb` and its macOS and
/// Windows equivalents, which all normalise to this set.
#[must_use]
pub fn named_key(key: &str) -> Option<Key> {
    Some(match key {
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "escape" | "esc" => Key::Escape,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "prior" => Key::PageUp,
        "pagedown" | "next" => Key::PageDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        other => {
            let number = other.strip_prefix('f')?;
            // `f` on its own is the letter, not a function key.
            if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Key::Function(number.parse().ok()?)
        }
    })
}

/// The text a keystroke committed, when it is text at all.
///
/// Ctrl and Super chords never produce text — the platform still fills
/// `key_char` for some of them, and honouring it would send `^C` as the letter
/// `c`.
fn committed_text(key_char: Option<&str>, mods: Mods) -> Option<String> {
    if mods.intersects(Mods::CTRL | Mods::SUPER) {
        return None;
    }
    let text = key_char?;
    if text.is_empty() || text.chars().any(char::is_control) {
        return None;
    }
    Some(text.to_string())
}

/// The single character a keystroke stands for.
///
/// Prefers `key_char` so a shifted or Alt-composed glyph reaches the PTY
/// unchanged, and falls back to `key` for Ctrl chords, where `key` is the
/// unshifted letter the control table is indexed by.
#[must_use]
pub fn character(key: &str, key_char: Option<&str>, mods: Mods) -> Option<char> {
    if key == "space" {
        return Some(' ');
    }
    if let Some(text) = committed_text(key_char, mods) {
        let mut chars = text.chars();
        if let (Some(ch), None) = (chars.next(), chars.next()) {
            return Some(ch);
        }
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Some(ch),
        _ => None,
    }
}

/// gpui modifier flags → `st-client-core` modifier flags.
#[must_use]
pub fn mods_from_gpui(modifiers: gpui::Modifiers) -> Mods {
    let mut mods = Mods::empty();
    mods.set(Mods::CTRL, modifiers.control);
    mods.set(Mods::ALT, modifiers.alt);
    mods.set(Mods::SHIFT, modifiers.shift);
    mods.set(Mods::SUPER, modifiers.platform);
    mods
}

/// The chord string a `shortcut` event carries, in the spelling
/// `passthroughKeys` uses, so React can look it up in its command registry
/// without re-deriving the normalisation.
#[must_use]
pub fn chord_string(mods: Mods, key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 16);
    for (flag, name) in [
        (Mods::CTRL, "ctrl"),
        (Mods::ALT, "alt"),
        (Mods::SHIFT, "shift"),
        (Mods::SUPER, "cmd"),
    ] {
        if mods.contains(flag) {
            out.push_str(name);
            out.push('-');
        }
    }
    out.push_str(key);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pass(list: &[&str]) -> PassthroughKeys {
        PassthroughKeys::parse(&json!(list))
    }

    fn send(key: &str, key_char: Option<&str>, mods: Mods) -> Vec<u8> {
        match handle_key(
            key,
            key_char,
            mods,
            &PassthroughKeys::default(),
            Modes::empty(),
            &KeyConfig::default(),
        ) {
            KeyOutcome::Send(bytes) => bytes,
            other => panic!("expected bytes, got {other:?}"),
        }
    }

    #[test]
    fn a_plain_letter_is_its_own_byte() {
        assert_eq!(send("a", Some("a"), Mods::empty()), b"a");
    }

    #[test]
    fn shift_arrives_as_the_composed_character() {
        // gpui reports key="a", key_char="A" for shift-a.
        assert_eq!(send("a", Some("A"), Mods::SHIFT), b"A");
        // …and key="1", key_char="!" for shift-1.
        assert_eq!(send("1", Some("!"), Mods::SHIFT), b"!");
    }

    #[test]
    fn ctrl_uses_the_unshifted_key_not_the_character() {
        assert_eq!(send("c", Some("c"), Mods::CTRL), vec![0x03]);
        // Some platforms fill key_char with the control byte itself; the key
        // name is what the control table is indexed by.
        assert_eq!(send("c", Some("\u{3}"), Mods::CTRL), vec![0x03]);
        assert_eq!(send("space", None, Mods::CTRL), vec![0x00]);
    }

    #[test]
    fn alt_prefixes_with_escape() {
        assert_eq!(send("x", Some("x"), Mods::ALT), vec![ESC, b'x']);
    }

    #[test]
    fn named_keys_map_to_their_xterm_sequences() {
        assert_eq!(send("enter", None, Mods::empty()), b"\r");
        assert_eq!(send("tab", None, Mods::empty()), b"\t");
        assert_eq!(send("tab", None, Mods::SHIFT), b"\x1b[Z");
        assert_eq!(send("escape", None, Mods::empty()), vec![ESC]);
        assert_eq!(send("backspace", None, Mods::empty()), vec![0x7F]);
        assert_eq!(send("up", None, Mods::empty()), b"\x1b[A");
        assert_eq!(send("pageup", None, Mods::empty()), b"\x1b[5~");
        assert_eq!(send("f5", None, Mods::empty()), b"\x1b[15~");
        assert_eq!(send("f1", None, Mods::empty()), b"\x1bOP");
    }

    #[test]
    fn application_cursor_mode_reaches_the_encoder() {
        let outcome = handle_key(
            "up",
            None,
            Mods::empty(),
            &PassthroughKeys::default(),
            Modes::APP_CURSOR_KEYS,
            &KeyConfig::default(),
        );
        assert_eq!(outcome, KeyOutcome::Send(b"\x1bOA".to_vec()));
    }

    #[test]
    fn a_passthrough_chord_is_declined_before_anything_else() {
        let outcome = handle_key(
            "t",
            Some("t"),
            Mods::SUPER,
            &pass(&["cmd-t"]),
            Modes::empty(),
            &KeyConfig::default(),
        );
        assert_eq!(outcome, KeyOutcome::Passthrough);
    }

    #[test]
    fn a_passthrough_chord_wins_over_a_key_the_terminal_would_have_sent() {
        // ctrl-shift-c is a perfectly encodable chord, but if React claims it
        // the element must not send ^C.
        let outcome = handle_key(
            "c",
            Some("C"),
            Mods::CTRL | Mods::SHIFT,
            &pass(&["ctrl-shift-c"]),
            Modes::empty(),
            &KeyConfig::default(),
        );
        assert_eq!(outcome, KeyOutcome::Passthrough);

        let outcome = handle_key(
            "c",
            Some("C"),
            Mods::CTRL | Mods::SHIFT,
            &PassthroughKeys::default(),
            Modes::empty(),
            &KeyConfig::default(),
        );
        assert_eq!(outcome, KeyOutcome::Send(vec![0x03]));
    }

    #[test]
    fn an_unclaimed_super_chord_is_ignored_rather_than_sent() {
        let outcome = handle_key(
            "q",
            Some("q"),
            Mods::SUPER,
            &PassthroughKeys::default(),
            Modes::empty(),
            &KeyConfig::default(),
        );
        assert_eq!(outcome, KeyOutcome::Ignore);
    }

    #[test]
    fn modifier_only_and_unknown_keys_are_ignored() {
        for key in ["ctrl", "shift", "capslock", "f99", "unknownkey"] {
            let outcome = handle_key(
                key,
                None,
                Mods::empty(),
                &PassthroughKeys::default(),
                Modes::empty(),
                &KeyConfig::default(),
            );
            assert_eq!(outcome, KeyOutcome::Ignore, "{key}");
        }
    }

    #[test]
    fn an_ime_commit_of_several_characters_is_sent_as_text() {
        let outcome = handle_key(
            "a",
            Some("日本語"),
            Mods::empty(),
            &PassthroughKeys::default(),
            Modes::empty(),
            &KeyConfig::default(),
        );
        assert_eq!(outcome, KeyOutcome::Send("日本語".as_bytes().to_vec()));
    }

    #[test]
    fn f_alone_is_the_letter_not_a_function_key() {
        assert_eq!(named_key("f"), None);
        assert_eq!(send("f", Some("f"), Mods::empty()), b"f");
    }

    #[test]
    fn chord_strings_round_trip_through_the_chord_parser() {
        use crate::props::Chord;
        for (mods, key) in [
            (Mods::SUPER, "t"),
            (Mods::CTRL | Mods::SHIFT, "c"),
            (Mods::ALT, "1"),
            (Mods::empty(), "enter"),
        ] {
            let text = chord_string(mods, key);
            let chord = Chord::parse(&text).expect(&text);
            assert!(chord.matches(mods, key), "{text}");
        }
    }

    #[test]
    fn gpui_modifiers_translate_one_for_one() {
        let modifiers = gpui::Modifiers {
            control: true,
            alt: false,
            shift: true,
            platform: false,
            function: true,
        };
        assert_eq!(mods_from_gpui(modifiers), Mods::CTRL | Mods::SHIFT);
    }
}
