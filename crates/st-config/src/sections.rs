//! The typed sections of `config.toml`.
//!
//! Every section derives [`Default`] and is marked `#[serde(default)]`, so a
//! file may contain any subset of sections and any subset of keys. The Bun app
//! mirrors this schema with zod (`packages/app/src/config/schema.ts`); keep the
//! two in step — `docs/config-example.toml` is generated from this module and
//! is the shared fixture (Q46).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::color::Rgb;
use crate::paths::Platform;

/// `[font]` — the single monospaced family used for terminal cells (Q26).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    /// Family name. Unset means the platform's default monospace family; see
    /// [`FontConfig::resolved_family`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// Size in pixels.
    #[serde(with = "crate::num::f32_toml")]
    pub size: f32,
    /// Multiplier applied to the font size to get the cell height.
    #[serde(with = "crate::num::f32_toml")]
    pub line_height: f32,
}

impl FontConfig {
    /// Default family on macOS.
    pub const MACOS_DEFAULT_FAMILY: &'static str = "Menlo";
    /// Default family on Linux.
    pub const LINUX_DEFAULT_FAMILY: &'static str = "DejaVu Sans Mono";

    /// The configured family, or the platform default when unset.
    pub fn resolved_family(&self, platform: Platform) -> &str {
        match &self.family {
            Some(f) => f,
            None => match platform {
                Platform::MacOs => Self::MACOS_DEFAULT_FAMILY,
                Platform::Linux => Self::LINUX_DEFAULT_FAMILY,
            },
        }
    }

    /// Cell height in pixels: `size * line_height`.
    pub fn cell_height(&self) -> f32 {
        self.size * self.line_height
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: None,
            size: 13.0,
            line_height: 1.2,
        }
    }
}

/// How the window is composited (Q28).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowBackground {
    /// Decide at startup: blurred on macOS, opaque on Linux (with the client's
    /// WSLg/Wayland refinement in `05-client-app.md`).
    #[default]
    Auto,
    /// Translucent with a background blur. Treated as `transparent` on Linux.
    Blurred,
    /// Translucent, no blur.
    Transparent,
    /// Fully opaque.
    Opaque,
}

impl WindowBackground {
    /// Resolves [`WindowBackground::Auto`] (and `Blurred` on Linux, which has
    /// no blur) into a value the renderer can use directly.
    pub fn resolve(self, platform: Platform) -> Self {
        match (self, platform) {
            (Self::Auto, Platform::MacOs) => Self::Blurred,
            (Self::Auto, Platform::Linux) => Self::Opaque,
            (Self::Blurred, Platform::Linux) => Self::Transparent,
            (other, _) => other,
        }
    }

    /// Whether the resolved background lets the desktop show through.
    pub fn is_translucent(self) -> bool {
        matches!(self, Self::Blurred | Self::Transparent)
    }
}

/// `[window.padding]` — pixels of empty space between the window edge and the
/// first cell. The scrollbar is drawn inside the right padding.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Padding {
    /// Pixels above the first row.
    #[serde(with = "crate::num::f32_toml")]
    pub top: f32,
    /// Pixels right of the last column.
    #[serde(with = "crate::num::f32_toml")]
    pub right: f32,
    /// Pixels below the last row.
    #[serde(with = "crate::num::f32_toml")]
    pub bottom: f32,
    /// Pixels left of the first column.
    #[serde(with = "crate::num::f32_toml")]
    pub left: f32,
}

impl Default for Padding {
    fn default() -> Self {
        Self {
            top: 8.0,
            right: 8.0,
            bottom: 8.0,
            left: 8.0,
        }
    }
}

/// `[window]` — chrome and compositing.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    /// Compositing mode; see [`WindowBackground`].
    pub background: WindowBackground,
    /// Lay the tab strip out vertically down the left edge instead of
    /// horizontally along the top.
    pub vertical_tabs: bool,
    /// Padding around the cell grid.
    pub padding: Padding,
}

/// `[shell]` — what the server spawns in a new Surface (`03-server.md` §9).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    /// Program to run. Unset means `$SHELL`, then `/bin/sh`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// Extra arguments passed before the login flag.
    pub args: Vec<String>,
    /// Start the shell as a login shell (`-l`).
    ///
    /// Unset means the platform default: `true` on macOS, `false` on Linux
    /// (`03-server.md` §9). Kept optional so the serialised schema — and
    /// therefore `docs/config-example.toml`, the fixture the Bun app shares —
    /// is identical on both platforms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<bool>,
}

/// A shell command line ready to hand to `portable_pty::CommandBuilder`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedShell {
    /// The program to execute.
    pub program: PathBuf,
    /// Its arguments (not including `argv[0]`).
    pub args: Vec<String>,
}

impl ShellConfig {
    /// The last-resort shell when neither the config nor `$SHELL` names one.
    pub const FALLBACK_SHELL: &'static str = "/bin/sh";

    /// Shells that understand `-l`.
    const LOGIN_CAPABLE: [&'static str; 3] = ["bash", "zsh", "fish"];

    /// Whether the shell is started as a login shell on `platform`.
    pub fn login_enabled(&self, platform: Platform) -> bool {
        self.login.unwrap_or(matches!(platform, Platform::MacOs))
    }

    /// Resolves the command line for the current platform.
    ///
    /// `shell_env` is the value of `$SHELL`; pass `None` when it is unset so
    /// this stays a pure function.
    pub fn resolve(&self, shell_env: Option<&str>) -> ResolvedShell {
        self.resolve_on(Platform::current(), shell_env)
    }

    /// Resolves the command line: `[shell].program` → `$SHELL` → `/bin/sh`,
    /// appending `-l` when [`ShellConfig::login_enabled`] and the program is a
    /// shell that understands it.
    pub fn resolve_on(&self, platform: Platform, shell_env: Option<&str>) -> ResolvedShell {
        let program = self
            .program
            .as_deref()
            .filter(|p| !p.is_empty())
            .or(shell_env.filter(|p| !p.is_empty()))
            .unwrap_or(Self::FALLBACK_SHELL);
        let program = PathBuf::from(program);

        let mut args = self.args.clone();
        if self.login_enabled(platform)
            && Self::accepts_login_flag(&program)
            && !args.iter().any(|a| a == "-l")
        {
            args.push("-l".to_owned());
        }
        ResolvedShell { program, args }
    }

    /// Whether appending `-l` is meaningful for this program.
    fn accepts_login_flag(program: &std::path::Path) -> bool {
        program
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| Self::LOGIN_CAPABLE.contains(&name))
    }
}

/// What the macOS Option key does (`04-client-native.md` §7). Ignored on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptionAsAlt {
    /// Option composes characters as macOS normally does.
    #[default]
    None,
    /// Only the left Option key acts as Alt (prefixes `ESC`).
    Left,
    /// Only the right Option key acts as Alt.
    Right,
    /// Both Option keys act as Alt.
    Both,
}

/// What the Backspace key sends (`04-client-native.md` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackspaceSends {
    /// `0x7F` (DEL) — the xterm default.
    #[default]
    Del,
    /// `0x08` (BS / `^H`).
    Bs,
}

impl BackspaceSends {
    /// The byte this setting sends.
    pub fn byte(self) -> u8 {
        match self {
            Self::Del => 0x7f,
            Self::Bs => 0x08,
        }
    }
}

/// `[terminal]` — emulation and interaction knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// Lines of history kept per Surface. Clamped to
    /// [`TerminalConfig::MAX_SCROLLBACK_LINES`].
    pub scrollback_lines: usize,
    /// Render indexed colours 0-7 with their bright variant when the cell is
    /// bold (Q48; default off).
    pub bold_is_bright: bool,
    /// Translate wheel events into arrow keys while the alternate screen is
    /// active, so pagers scroll.
    pub alt_screen_scroll: bool,
    /// Characters that count as part of a word — in addition to letters and
    /// digits — when double-clicking to select.
    pub word_chars: String,
    /// macOS only: whether Option behaves as Alt.
    pub option_as_alt: OptionAsAlt,
    /// The byte Backspace sends.
    pub backspace_sends: BackspaceSends,
}

impl TerminalConfig {
    /// Default value of [`TerminalConfig::scrollback_lines`].
    pub const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
    /// Upper bound enforced on [`TerminalConfig::scrollback_lines`]
    /// (`03-server.md` §4); larger values are clamped with a warning.
    pub const MAX_SCROLLBACK_LINES: usize = 100_000;
    /// Default value of [`TerminalConfig::word_chars`].
    pub const DEFAULT_WORD_CHARS: &'static str = "/-+\\~_.";

    /// Whether `c` belongs to a word for double-click selection.
    pub fn is_word_char(&self, c: char) -> bool {
        c.is_alphanumeric() || self.word_chars.contains(c)
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: Self::DEFAULT_SCROLLBACK_LINES,
            bold_is_bright: false,
            alt_screen_scroll: true,
            word_chars: Self::DEFAULT_WORD_CHARS.to_owned(),
            option_as_alt: OptionAsAlt::default(),
            backspace_sends: BackspaceSends::default(),
        }
    }
}

/// `[server]` — the daemon's own behaviour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Minutes of no connections and no non-pristine Surfaces before the
    /// server exits (Q30/Q42). `0` disables idle exit.
    #[serde(with = "crate::num::f64_toml")]
    pub idle_exit_minutes: f64,
    /// Honour OSC 52 clipboard writes from programs. Off in v1 (Q48).
    pub osc52: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            idle_exit_minutes: 15.0,
            osc52: false,
        }
    }
}

/// `[theme]` — the terminal palette (Q48: also used by the server to answer
/// OSC 10/11 colour queries).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Default text colour, and the answer to OSC 10.
    pub foreground: Rgb,
    /// Default background colour, and the answer to OSC 11.
    pub background: Rgb,
    /// Cursor block colour.
    pub cursor: Rgb,
    /// Colour of the character underneath a block cursor.
    pub cursor_text: Rgb,
    /// Background of selected cells.
    pub selection_background: Rgb,
    /// Foreground of selected cells.
    pub selection_foreground: Rgb,
    /// ANSI 0.
    pub black: Rgb,
    /// ANSI 1.
    pub red: Rgb,
    /// ANSI 2.
    pub green: Rgb,
    /// ANSI 3.
    pub yellow: Rgb,
    /// ANSI 4.
    pub blue: Rgb,
    /// ANSI 5.
    pub magenta: Rgb,
    /// ANSI 6.
    pub cyan: Rgb,
    /// ANSI 7.
    pub white: Rgb,
    /// ANSI 8.
    pub bright_black: Rgb,
    /// ANSI 9.
    pub bright_red: Rgb,
    /// ANSI 10.
    pub bright_green: Rgb,
    /// ANSI 11.
    pub bright_yellow: Rgb,
    /// ANSI 12.
    pub bright_blue: Rgb,
    /// ANSI 13.
    pub bright_magenta: Rgb,
    /// ANSI 14.
    pub bright_cyan: Rgb,
    /// ANSI 15.
    pub bright_white: Rgb,
}

impl ThemeConfig {
    /// The 16 ANSI colours in index order, ready for
    /// `st-client-core`'s 256-entry palette table.
    pub fn ansi(&self) -> [Rgb; 16] {
        [
            self.black,
            self.red,
            self.green,
            self.yellow,
            self.blue,
            self.magenta,
            self.cyan,
            self.white,
            self.bright_black,
            self.bright_red,
            self.bright_green,
            self.bright_yellow,
            self.bright_blue,
            self.bright_magenta,
            self.bright_cyan,
            self.bright_white,
        ]
    }

    /// The name of ANSI colour `index` (0-15) as it appears in `[theme]`.
    pub fn ansi_key(index: u8) -> Option<&'static str> {
        const KEYS: [&str; 16] = [
            "black",
            "red",
            "green",
            "yellow",
            "blue",
            "magenta",
            "cyan",
            "white",
            "bright_black",
            "bright_red",
            "bright_green",
            "bright_yellow",
            "bright_blue",
            "bright_magenta",
            "bright_cyan",
            "bright_white",
        ];
        KEYS.get(index as usize).copied()
    }
}

impl Default for ThemeConfig {
    /// The built-in neutral dark palette (`04-client-native.md` §10:
    /// background `#1e1e1e`, foreground `#d4d4d4`).
    fn default() -> Self {
        Self {
            foreground: Rgb::from_u32(0xd4d4d4),
            background: Rgb::from_u32(0x1e1e1e),
            cursor: Rgb::from_u32(0xd4d4d4),
            cursor_text: Rgb::from_u32(0x1e1e1e),
            selection_background: Rgb::from_u32(0x264f78),
            selection_foreground: Rgb::from_u32(0xd4d4d4),
            black: Rgb::from_u32(0x000000),
            red: Rgb::from_u32(0xcd3131),
            green: Rgb::from_u32(0x0dbc79),
            yellow: Rgb::from_u32(0xe5e510),
            blue: Rgb::from_u32(0x2472c8),
            magenta: Rgb::from_u32(0xbc3fbc),
            cyan: Rgb::from_u32(0x11a8cd),
            white: Rgb::from_u32(0xe5e5e5),
            bright_black: Rgb::from_u32(0x666666),
            bright_red: Rgb::from_u32(0xf14c4c),
            bright_green: Rgb::from_u32(0x23d18b),
            bright_yellow: Rgb::from_u32(0xf5f543),
            bright_blue: Rgb::from_u32(0x3b8eea),
            bright_magenta: Rgb::from_u32(0xd670d6),
            bright_cyan: Rgb::from_u32(0x29b8db),
            bright_white: Rgb::from_u32(0xffffff),
        }
    }
}

/// `[keybindings]` — command id → shortcut string (`"mod+shift+t"`).
///
/// A `BTreeMap` so serialisation is deterministic. Entries *override* the
/// built-in table in `05-client-app.md` §5; the empty map means "defaults".
pub type Keybindings = BTreeMap<String, String>;
