//! Dropping files onto the grid: each path becomes one shell word, typed.
//!
//! A file dragged from Finder or Explorer onto a terminal is not a transfer.
//! iTerm2 and Terminal.app type the file's **path**, escaped so the shell
//! reads it as one word, and follow it with a space; several files become
//! several words. That is how a TUI such as OpenCode turns a dropped PNG into
//! an attachment: it is parsing a pasted path.
//!
//! Pure and headless-tested. The platform hop — the drag session, the drop
//! event — is GPUI's, wired in `element.rs`.
//!
//! **Escaping.** POSIX shells get backslashes rather than quotes: a quoted
//! path is harder for a program reading stdin to recognise as a path, and
//! every POSIX shell accepts the backslash form (iTerm's own issue #918 is
//! about exactly which characters). Non-ASCII is never escaped — the shell
//! takes UTF-8 as-is, and escaping `é` would only corrupt it. Windows shells
//! (cmd, PowerShell) do not understand backslash escapes at all, so there a
//! path with anything outside the safe set is double-quoted instead.

use std::path::Path;

/// Which shell dialect the typed word has to survive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// sh, bash, zsh, fish: backslash before every special ASCII byte.
    Posix,
    /// cmd.exe and PowerShell: double quotes around the whole word.
    Windows,
}

impl Flavor {
    /// The dialect for the platform this addon was built for.
    #[must_use]
    pub const fn native() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Posix
        }
    }
}

/// `true` when `byte` must be escaped for a POSIX shell.
///
/// Safe, left alone: ASCII letters, digits, and `. _ / - + , : @ = %`.
/// Everything else in ASCII gets a backslash — space, both quotes, brackets
/// and braces, `$ ~ # ! & ; | < > * ?`, backslash itself, newline. Bytes
/// outside ASCII are part of a UTF-8 sequence and are never escaped.
#[must_use]
pub fn needs_escape(byte: u8) -> bool {
    if !byte.is_ascii() {
        return false;
    }
    !(byte.is_ascii_alphanumeric() || b"._/-+,:@=%".contains(&byte))
}

/// One path as a shell word, plus the trailing space that separates it from
/// whatever the user types next.
#[must_use]
pub fn shell_word(path: &Path) -> String {
    shell_word_for(path, Flavor::native())
}

/// [`shell_word`] for an explicit dialect.
#[must_use]
pub fn shell_word_for(path: &Path, flavor: Flavor) -> String {
    let raw = path.to_string_lossy();
    let mut out = String::with_capacity(raw.len() + 2);
    match flavor {
        Flavor::Posix => {
            for ch in raw.chars() {
                if ch.is_ascii() && needs_escape(ch as u8) {
                    out.push('\\');
                }
                out.push(ch);
            }
        }
        Flavor::Windows => {
            // `\` and `:` are ordinary path characters on Windows, so the safe
            // set is the POSIX one plus those; anything else means quoting.
            let plain = raw
                .bytes()
                .all(|b| !b.is_ascii() || !needs_escape(b) || b == b'\\');
            if plain {
                out.push_str(&raw);
            } else {
                out.push('"');
                for ch in raw.chars() {
                    if ch == '"' {
                        // cmd has no escape for a quote inside quotes; `""`
                        // is what PowerShell reads, and a quote in a file
                        // name is illegal on NTFS anyway.
                        out.push('"');
                    }
                    out.push(ch);
                }
                out.push('"');
            }
        }
    }
    out.push(' ');
    out
}

/// Every dropped path in drop order, each as a [`shell_word`].
#[must_use]
pub fn shell_words<P: AsRef<Path>>(paths: &[P]) -> String {
    shell_words_for(paths, Flavor::native())
}

/// [`shell_words`] for an explicit dialect.
#[must_use]
pub fn shell_words_for<P: AsRef<Path>>(paths: &[P], flavor: Flavor) -> String {
    paths
        .iter()
        .map(|path| shell_word_for(path.as_ref(), flavor))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn posix(path: &str) -> String {
        shell_word_for(Path::new(path), Flavor::Posix)
    }

    #[test]
    fn a_plain_path_is_typed_verbatim_with_a_trailing_space() {
        assert_eq!(posix("/Users/me/shot.png"), "/Users/me/shot.png ");
        assert_eq!(posix("./a-b_c.d+e,f:g@h=i%j"), "./a-b_c.d+e,f:g@h=i%j ");
    }

    #[test]
    fn spaces_and_shell_metacharacters_get_a_backslash() {
        assert_eq!(posix("/Users/me/My Shot.png"), "/Users/me/My\\ Shot.png ");
        assert_eq!(posix("a'b\"c"), "a\\'b\\\"c ");
        assert_eq!(posix("x(1) [2] {3}"), "x\\(1\\)\\ \\[2\\]\\ \\{3\\} ");
        assert_eq!(posix("$HOME~#!&;|<>*?"), "\\$HOME\\~\\#\\!\\&\\;\\|\\<\\>\\*\\? ");
        assert_eq!(posix("back\\slash"), "back\\\\slash ");
        assert_eq!(posix("new\nline"), "new\\\nline ");
    }

    #[test]
    fn non_ascii_is_left_alone_because_the_shell_takes_utf8_as_is() {
        assert_eq!(posix("/tmp/café.png"), "/tmp/café.png ");
        assert_eq!(posix("/tmp/日本語 x.txt"), "/tmp/日本語\\ x.txt ");
        for byte in 0x80..=0xFFu8 {
            assert!(!needs_escape(byte), "{byte:#x} is inside a UTF-8 sequence");
        }
    }

    #[test]
    fn the_safe_set_is_exactly_the_documented_one() {
        for byte in 0..=0x7Fu8 {
            let safe = byte.is_ascii_alphanumeric() || b"._/-+,:@=%".contains(&byte);
            assert_eq!(needs_escape(byte), !safe, "{:?}", byte as char);
        }
    }

    #[test]
    fn several_files_become_several_words_in_drop_order() {
        let paths = [PathBuf::from("/a/one.png"), PathBuf::from("/b/two three.png")];
        assert_eq!(
            shell_words_for(&paths, Flavor::Posix),
            "/a/one.png /b/two\\ three.png "
        );
        assert_eq!(shell_words_for(&[] as &[PathBuf], Flavor::Posix), "");
    }

    #[test]
    fn a_windows_path_keeps_its_backslashes_and_quotes_only_when_it_must() {
        let win = |p: &str| shell_word_for(Path::new(p), Flavor::Windows);
        assert_eq!(win("C:\\Users\\me\\shot.png"), "C:\\Users\\me\\shot.png ");
        assert_eq!(win("C:\\Users\\me\\My Shot.png"), "\"C:\\Users\\me\\My Shot.png\" ");
        assert_eq!(win("C:\\x\\a(1).png"), "\"C:\\x\\a(1).png\" ");
        assert_eq!(win("C:\\tmp\\café.png"), "C:\\tmp\\café.png ");
    }

    #[test]
    fn the_native_flavor_matches_the_build_target() {
        assert_eq!(Flavor::native() == Flavor::Windows, cfg!(windows));
        let word = shell_word(Path::new("plain.txt"));
        assert_eq!(word, "plain.txt ");
    }
}
