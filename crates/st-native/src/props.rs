//! The `<terminal-grid>` prop surface (`docs/plan/04-client-native.md` §3).
//!
//! `set_prop` arrives as `(&str, serde_json::Value)` with no schema and no way
//! to report an error back to React, so every parser here is *total*: a bad
//! value keeps the previous one and the frame still paints. The one thing a
//! parser may not do is panic — it runs on the GPUI thread.

use st_client_core::keys::{KeyConfig, Mods};
use st_client_core::mouse::{AltScreenScroll, WheelConfig};
use st_client_core::palette::Palette;
use st_client_core::selection::SelectionConfig;
use st_proto::AttachMode;

use crate::theme::{default_palette, palette_from_json};

/// Every key `supported_props()` declares. Order is the order the registry
/// applies them in, which matters only in that `theme` must be able to land
/// before the first paint.
pub const SUPPORTED_PROPS: &[&str] = &[
    "surfaceId",
    "socketPath",
    "buildId",
    "attachMode",
    "fontFamily",
    "fontSize",
    "lineHeight",
    "theme",
    "cursorStyle",
    "cursorBlink",
    "padding",
    "passthroughKeys",
    "scrollbar",
    "focused",
    "wordChars",
    "altScreenScroll",
    "backspaceSendsDel",
    "altSendsEsc",
    "command",
];

/// Events the element can emit. The registry filters React's listener set down
/// to this list, so anything missing here is silently never delivered.
pub const SUPPORTED_EVENTS: &[&str] = &[
    "title",
    "bell",
    "exited",
    "selection",
    "scroll",
    "resize",
    "modes",
    "shortcut",
    "focus",
    "blur",
];

/// Default monospace stack. `.SystemUIFont` is gpui's own alias and is *not*
/// monospace, so we never fall back to it.
pub const DEFAULT_FONT_FAMILY: &str = "monospace";
/// 04 §6: `line_h = font_px * line_height`.
pub const DEFAULT_LINE_HEIGHT: f32 = 1.2;
/// Matches the default in `packages/app`'s config schema.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;

/// Cursor shape used when the program has not set DECSCUSR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorStyle {
    /// Filled cell with the glyph repainted in `cursorText`.
    #[default]
    Block,
    /// 2 px bar at the left cell edge.
    Beam,
    /// 2 px bar at the bottom cell edge.
    Underline,
}

impl CursorStyle {
    /// `"block" | "beam" | "underline"`, anything else keeps the default.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "block" => Some(Self::Block),
            "beam" | "bar" => Some(Self::Beam),
            "underline" | "underscore" => Some(Self::Underline),
            _ => None,
        }
    }

    /// The wire shape the server reports through `Cursor.shape`. `Cursor`
    /// carries visibility separately, so every shape maps.
    #[must_use]
    pub fn from_wire(shape: st_proto::CursorShape) -> Self {
        use st_proto::CursorShape;
        match shape {
            CursorShape::Block => Self::Block,
            CursorShape::Beam => Self::Beam,
            CursorShape::Underline => Self::Underline,
        }
    }
}

/// When the scrollbar is painted (04 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollbarMode {
    /// Only while the content overflows *and* the mouse moved recently.
    #[default]
    Auto,
    /// Whenever the content overflows.
    Always,
    /// Never.
    Never,
}

impl ScrollbarMode {
    /// `"auto" | "always" | "never"`.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" | "off" | "none" => Some(Self::Never),
            _ => None,
        }
    }
}

/// Inset, in pixels, between the element bounds and the first cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Padding {
    /// Top inset.
    pub top: f32,
    /// Right inset — the scrollbar lives here.
    pub right: f32,
    /// Bottom inset.
    pub bottom: f32,
    /// Left inset.
    pub left: f32,
}

impl Default for Padding {
    fn default() -> Self {
        Self {
            top: 4.0,
            right: 4.0,
            bottom: 4.0,
            left: 6.0,
        }
    }
}

impl Padding {
    /// `{top,right,bottom,left}`, a single number for all four sides, or
    /// `[v, h]` / `[t, r, b, l]` the way CSS shorthand reads.
    #[must_use]
    pub fn parse(value: &serde_json::Value) -> Option<Self> {
        let clamp = |n: f64| (n as f32).clamp(0.0, 512.0);
        if let Some(all) = value.as_f64() {
            let all = clamp(all);
            return Some(Self {
                top: all,
                right: all,
                bottom: all,
                left: all,
            });
        }
        if let Some(list) = value.as_array() {
            let n = |i: usize| list.get(i).and_then(serde_json::Value::as_f64).map(clamp);
            return match list.len() {
                2 => Some(Self {
                    top: n(0)?,
                    right: n(1)?,
                    bottom: n(0)?,
                    left: n(1)?,
                }),
                4 => Some(Self {
                    top: n(0)?,
                    right: n(1)?,
                    bottom: n(2)?,
                    left: n(3)?,
                }),
                _ => None,
            };
        }
        let object = value.as_object()?;
        let default = Self::default();
        let side = |key: &str, fallback: f32| {
            object
                .get(key)
                .and_then(serde_json::Value::as_f64)
                .map_or(fallback, clamp)
        };
        Some(Self {
            top: side("top", default.top),
            right: side("right", default.right),
            bottom: side("bottom", default.bottom),
            left: side("left", default.left),
        })
    }
}

/// A one-shot imperative call delivered as a prop (04 §3).
///
/// The retained tree has no method calls, so React bumps `seq` and the element
/// runs the command once. Re-rendering with the same `seq` must do nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// Monotonically increasing; the element ignores anything it has seen.
    pub seq: u64,
    /// `copy`, `paste`, `clearScrollback`, `scrollToBottom`, `selectAll`,
    /// `clearSelection`.
    pub name: String,
    /// Optional payload; only `paste` uses it (text to paste instead of the
    /// clipboard).
    pub text: Option<String>,
}

impl Command {
    /// `{ seq, name, text? }`.
    #[must_use]
    pub fn parse(value: &serde_json::Value) -> Option<Self> {
        let object = value.as_object()?;
        Some(Self {
            seq: object.get("seq").and_then(serde_json::Value::as_u64)?,
            name: object.get("name")?.as_str()?.to_string(),
            text: object
                .get("text")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        })
    }
}

/// A chord React claims for itself (04 §7, grilling Q23).
///
/// Stored normalised so matching is a comparison of two small values rather
/// than string munging on the key path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Chord {
    /// Modifiers, in `st-client-core` terms so the same type serves encoding.
    pub mods: Mods,
    /// The gpui key name, lower-cased. `None` is the `*` wildcard: "any key
    /// with exactly these modifiers", which is how the macOS default
    /// `"cmd-*"` is expressed.
    pub key: Option<String>,
}

impl Chord {
    /// Parses one gpuix keystroke string: `"cmd-shift-]"`, `"ctrl-shift-t"`,
    /// `"alt-1"`, `"cmd-*"`. Modifier order and case do not matter.
    ///
    /// The separator is `-`, which is also a key name, so the *last* segment is
    /// always the key: `"ctrl--"` is Ctrl plus the minus key.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let lower = raw.to_ascii_lowercase();
        let mut segments: Vec<&str> = lower.split('-').collect();
        // "ctrl--" splits to ["ctrl", "", ""]: the trailing empty segment is
        // the literal `-` key and the one before it is the separator.
        if segments.last().is_some_and(|s| s.is_empty()) && segments.len() > 1 {
            segments.pop();
            if let Some(last) = segments.last_mut() {
                *last = "-";
            }
        }
        let key = segments.pop()?;
        let mut mods = Mods::empty();
        for segment in segments {
            mods |= match segment {
                "ctrl" | "control" => Mods::CTRL,
                "alt" | "opt" | "option" => Mods::ALT,
                "shift" => Mods::SHIFT,
                "cmd" | "command" | "super" | "meta" | "win" | "platform" => Mods::SUPER,
                "fn" | "function" => Mods::empty(),
                _ => return None,
            };
        }
        Some(Self {
            mods,
            key: (key != "*").then(|| key.to_string()),
        })
    }

    /// Does this chord claim `(mods, key)`?
    #[must_use]
    pub fn matches(&self, mods: Mods, key: &str) -> bool {
        if self.mods != mods {
            return false;
        }
        match &self.key {
            None => true,
            Some(expected) => expected.eq_ignore_ascii_case(key),
        }
    }
}

/// The parsed `passthroughKeys` prop.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PassthroughKeys(Vec<Chord>);

impl PassthroughKeys {
    /// Builds from the JSON array React sends. Non-strings and unparseable
    /// chords are dropped rather than poisoning the whole list — one typo in
    /// the command registry must not make the terminal swallow `cmd-q`.
    #[must_use]
    pub fn parse(value: &serde_json::Value) -> Self {
        let Some(list) = value.as_array() else {
            return Self::default();
        };
        Self(
            list.iter()
                .filter_map(|entry| entry.as_str())
                .filter_map(Chord::parse)
                .collect(),
        )
    }

    /// Builds from already-normalised chords, for the platform defaults.
    #[must_use]
    pub fn from_chords(chords: Vec<Chord>) -> Self {
        Self(chords)
    }

    /// `true` when the element must decline `(mods, key)` so it reaches React.
    #[must_use]
    pub fn contains(&self, mods: Mods, key: &str) -> bool {
        self.0.iter().any(|chord| chord.matches(mods, key))
    }

    /// How many chords are claimed. Exposed for `stats`.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` when React claimed nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What changed when a prop was applied, so `render()` knows what to redo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PropEffect {
    /// Font family/size/line height changed: recompute metrics, drop the run
    /// cache, and re-derive the grid size (04 §3).
    pub metrics: bool,
    /// The palette changed: the run cache keys colour, so it is stale.
    pub theme: bool,
    /// `surfaceId`, `socketPath` or `attachMode` changed: re-attach.
    pub attach: bool,
    /// A `command` prop with an unseen `seq` arrived.
    pub command: bool,
    /// React changed the `focused` prop.
    pub focus: bool,
}

impl PropEffect {
    /// Merges two effects.
    #[must_use]
    pub fn or(self, other: Self) -> Self {
        Self {
            metrics: self.metrics || other.metrics,
            theme: self.theme || other.theme,
            attach: self.attach || other.attach,
            command: self.command || other.command,
            focus: self.focus || other.focus,
        }
    }

    /// `true` when nothing needs redoing.
    #[must_use]
    pub fn is_noop(self) -> bool {
        self == Self::default()
    }
}

/// Everything React can set on one `<terminal-grid>`.
#[derive(Debug, Clone)]
pub struct GridProps {
    /// Which Surface to attach to. `None` until React supplies one.
    pub surface_id: Option<u32>,
    /// Data Plane socket. `None` until React supplies one.
    pub socket_path: Option<String>,
    /// Sent in `Hello`; identifies the client build to the server.
    pub build_id: String,
    /// Active (rows) or Passive (metadata only) — grilling Q44.
    pub attach_mode: AttachMode,
    /// Font family passed straight to gpui's font resolver.
    pub font_family: String,
    /// Font size in px, clamped to a sane range.
    pub font_size: f32,
    /// Multiplier on `font_size`, not an absolute height.
    pub line_height: f32,
    /// Resolved palette.
    pub palette: Palette,
    /// Fallback cursor shape.
    pub cursor_style: CursorStyle,
    /// Fallback blink flag.
    pub cursor_blink: bool,
    /// Inset around the grid.
    pub padding: Padding,
    /// Chords the element declines (04 §7).
    pub passthrough: PassthroughKeys,
    /// Scrollbar visibility policy.
    pub scrollbar: ScrollbarMode,
    /// React asking for focus (04 §3). `None` means "React is not driving
    /// focus"; the element still focuses itself on mouse-down either way.
    pub focused: Option<bool>,
    /// Key-encoding knobs (Backspace, Alt-as-Esc, paste newline).
    pub key_config: KeyConfig,
    /// Word characters and trailing-whitespace trimming for selection.
    pub selection_config: SelectionConfig,
    /// Wheel behaviour, including alt-screen arrow emulation.
    pub wheel_config: WheelConfig,
    /// The last command seen, so a re-render does not run it twice.
    pub command_seq: u64,
}

impl Default for GridProps {
    fn default() -> Self {
        Self {
            surface_id: None,
            socket_path: None,
            build_id: format!("st-native/{}", env!("CARGO_PKG_VERSION")),
            attach_mode: AttachMode::Active,
            font_family: DEFAULT_FONT_FAMILY.to_string(),
            font_size: DEFAULT_FONT_SIZE,
            line_height: DEFAULT_LINE_HEIGHT,
            palette: default_palette(),
            cursor_style: CursorStyle::default(),
            cursor_blink: true,
            padding: Padding::default(),
            passthrough: PassthroughKeys::default(),
            scrollbar: ScrollbarMode::default(),
            focused: None,
            key_config: KeyConfig::default(),
            selection_config: SelectionConfig::default(),
            wheel_config: WheelConfig::default(),
            command_seq: 0,
        }
    }
}

/// Sanity bounds. A `fontSize` of 0 divides by zero in the grid maths and a
/// `fontSize` of 10 000 allocates a glyph atlas the GPU refuses.
const MIN_FONT_SIZE: f32 = 4.0;
const MAX_FONT_SIZE: f32 = 200.0;
const MIN_LINE_HEIGHT: f32 = 0.8;
const MAX_LINE_HEIGHT: f32 = 4.0;

impl GridProps {
    /// Applies one prop. Returns what the caller has to redo, and the command
    /// to run when one arrived.
    pub fn set(&mut self, key: &str, value: &serde_json::Value) -> (PropEffect, Option<Command>) {
        let mut effect = PropEffect::default();
        let mut command = None;
        match key {
            "surfaceId" => {
                let next = value
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .or_else(|| value.as_str().and_then(|s| s.parse().ok()));
                if next != self.surface_id {
                    self.surface_id = next;
                    effect.attach = true;
                }
            }
            "socketPath" => {
                let next = value.as_str().filter(|s| !s.is_empty()).map(str::to_string);
                if next != self.socket_path {
                    self.socket_path = next;
                    effect.attach = true;
                }
            }
            "buildId" => {
                if let Some(id) = value.as_str().filter(|s| !s.is_empty()) {
                    self.build_id = id.to_string();
                }
            }
            "attachMode" => {
                let next = match value.as_str().map(str::to_ascii_lowercase).as_deref() {
                    Some("passive") => AttachMode::Passive,
                    _ => AttachMode::Active,
                };
                if next != self.attach_mode {
                    self.attach_mode = next;
                    effect.attach = true;
                }
            }
            "fontFamily" => {
                let next = value
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(DEFAULT_FONT_FAMILY)
                    .trim()
                    .to_string();
                if next != self.font_family {
                    self.font_family = next;
                    effect.metrics = true;
                }
            }
            "fontSize" => {
                let next = number(value, DEFAULT_FONT_SIZE).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
                if next != self.font_size {
                    self.font_size = next;
                    effect.metrics = true;
                }
            }
            "lineHeight" => {
                let next =
                    number(value, DEFAULT_LINE_HEIGHT).clamp(MIN_LINE_HEIGHT, MAX_LINE_HEIGHT);
                if next != self.line_height {
                    self.line_height = next;
                    effect.metrics = true;
                }
            }
            "theme" => {
                self.palette = palette_from_json(value);
                effect.theme = true;
            }
            "cursorStyle" => {
                if let Some(style) = value.as_str().and_then(CursorStyle::parse) {
                    self.cursor_style = style;
                } else {
                    self.cursor_style = CursorStyle::default();
                }
            }
            "cursorBlink" => self.cursor_blink = value.as_bool().unwrap_or(true),
            "padding" => {
                let next = Padding::parse(value).unwrap_or_default();
                if next != self.padding {
                    self.padding = next;
                    effect.metrics = true;
                }
            }
            "passthroughKeys" => self.passthrough = PassthroughKeys::parse(value),
            "scrollbar" => {
                self.scrollbar = value
                    .as_str()
                    .and_then(ScrollbarMode::parse)
                    .unwrap_or_default();
            }
            "focused" => {
                let next = value.as_bool();
                if next != self.focused {
                    self.focused = next;
                    effect.focus = true;
                }
            }
            "wordChars" => {
                self.selection_config.word_chars = value.as_str().map_or_else(
                    || st_client_core::selection::DEFAULT_WORD_CHARS.to_string(),
                    str::to_string,
                );
            }
            "altScreenScroll" => {
                self.wheel_config.alt_screen_scroll =
                    match value.as_str().map(str::to_ascii_lowercase).as_deref() {
                        Some("off") | Some("none") => AltScreenScroll::Off,
                        _ => AltScreenScroll::Arrows,
                    };
            }
            "backspaceSendsDel" => {
                self.key_config.backspace_sends_del = value.as_bool().unwrap_or(true);
            }
            "altSendsEsc" => self.key_config.alt_sends_esc = value.as_bool().unwrap_or(true),
            "command" => {
                if let Some(parsed) = Command::parse(value) {
                    if parsed.seq > self.command_seq {
                        self.command_seq = parsed.seq;
                        effect.command = true;
                        command = Some(parsed);
                    }
                }
            }
            _ => {}
        }
        (effect, command)
    }

    /// The key the shaped-run cache is invalidated on: anything that changes
    /// glyph geometry.
    #[must_use]
    pub fn font_key(&self) -> (String, u32, u32) {
        (
            self.font_family.clone(),
            self.font_size.to_bits(),
            self.line_height.to_bits(),
        )
    }
}

/// A JSON number, accepting the string form React sometimes produces from an
/// input field.
fn number(value: &serde_json::Value, fallback: f32) -> f32 {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
        .map_or(fallback, |n| n as f32)
        .max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chords_parse_regardless_of_order_or_case() {
        let a = Chord::parse("Cmd-Shift-T").unwrap();
        let b = Chord::parse("shift-cmd-t").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.mods, Mods::SUPER | Mods::SHIFT);
        assert_eq!(a.key.as_deref(), Some("t"));
    }

    #[test]
    fn the_last_segment_is_the_key_even_when_it_is_a_dash() {
        let chord = Chord::parse("ctrl--").unwrap();
        assert_eq!(chord.mods, Mods::CTRL);
        assert_eq!(chord.key.as_deref(), Some("-"));
        assert!(chord.matches(Mods::CTRL, "-"));
    }

    #[test]
    fn a_bracket_chord_survives_the_split() {
        let chord = Chord::parse("cmd-shift-]").unwrap();
        assert_eq!(chord.key.as_deref(), Some("]"));
        assert!(chord.matches(Mods::SUPER | Mods::SHIFT, "]"));
    }

    #[test]
    fn a_star_key_claims_every_key_with_those_exact_modifiers() {
        let chord = Chord::parse("cmd-*").unwrap();
        assert!(chord.matches(Mods::SUPER, "t"));
        assert!(chord.matches(Mods::SUPER, "enter"));
        // Exactly those modifiers: cmd-shift-t is a different chord.
        assert!(!chord.matches(Mods::SUPER | Mods::SHIFT, "t"));
        assert!(!chord.matches(Mods::empty(), "t"));
    }

    #[test]
    fn an_unknown_modifier_is_not_a_chord() {
        assert!(Chord::parse("hyper-t").is_none());
        assert!(Chord::parse("").is_none());
        assert!(Chord::parse("   ").is_none());
    }

    #[test]
    fn passthrough_drops_bad_entries_but_keeps_good_ones() {
        let keys = PassthroughKeys::parse(&json!(["cmd-t", 42, "hyper-x", "ctrl-shift-c"]));
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(Mods::SUPER, "t"));
        assert!(keys.contains(Mods::CTRL | Mods::SHIFT, "c"));
        assert!(!keys.contains(Mods::CTRL, "c"));
    }

    #[test]
    fn passthrough_of_a_non_array_claims_nothing() {
        assert!(PassthroughKeys::parse(&json!("cmd-t")).is_empty());
        assert!(PassthroughKeys::parse(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn font_props_clamp_and_report_a_metrics_change() {
        let mut props = GridProps::default();
        let (effect, _) = props.set("fontSize", &json!(0));
        assert!(effect.metrics);
        assert_eq!(props.font_size, MIN_FONT_SIZE);

        let (effect, _) = props.set("fontSize", &json!(1e9));
        assert!(effect.metrics);
        assert_eq!(props.font_size, MAX_FONT_SIZE);

        // Setting the same value again is not a change.
        let (effect, _) = props.set("fontSize", &json!(MAX_FONT_SIZE));
        assert!(effect.is_noop());
    }

    #[test]
    fn a_string_font_size_is_accepted() {
        let mut props = GridProps::default();
        props.set("fontSize", &json!("16"));
        assert_eq!(props.font_size, 16.0);
        props.set("fontSize", &json!("nonsense"));
        assert_eq!(props.font_size, DEFAULT_FONT_SIZE);
    }

    #[test]
    fn surface_and_socket_changes_ask_for_a_reattach() {
        let mut props = GridProps::default();
        let (effect, _) = props.set("surfaceId", &json!(7));
        assert!(effect.attach);
        assert_eq!(props.surface_id, Some(7));
        let (effect, _) = props.set("surfaceId", &json!(7));
        assert!(effect.is_noop());
        let (effect, _) = props.set("socketPath", &json!("/run/st/server.sock"));
        assert!(effect.attach);
        // Removal arrives as null and must detach, not keep the old surface.
        let (effect, _) = props.set("surfaceId", &serde_json::Value::Null);
        assert!(effect.attach);
        assert_eq!(props.surface_id, None);
    }

    #[test]
    fn padding_accepts_all_three_spellings() {
        assert_eq!(
            Padding::parse(&json!(8)).unwrap(),
            Padding {
                top: 8.0,
                right: 8.0,
                bottom: 8.0,
                left: 8.0
            }
        );
        assert_eq!(
            Padding::parse(&json!([2, 4])).unwrap(),
            Padding {
                top: 2.0,
                right: 4.0,
                bottom: 2.0,
                left: 4.0
            }
        );
        let object = Padding::parse(&json!({ "left": 12 })).unwrap();
        assert_eq!(object.left, 12.0);
        assert_eq!(object.top, Padding::default().top);
        assert!(Padding::parse(&json!([1, 2, 3])).is_none());
    }

    #[test]
    fn a_command_runs_once_per_sequence_number() {
        let mut props = GridProps::default();
        let (effect, command) = props.set("command", &json!({ "seq": 1, "name": "copy" }));
        assert!(effect.command);
        assert_eq!(command.unwrap().name, "copy");

        let (effect, command) = props.set("command", &json!({ "seq": 1, "name": "copy" }));
        assert!(effect.is_noop());
        assert!(command.is_none());

        let (effect, command) = props.set("command", &json!({ "seq": 2, "name": "paste" }));
        assert!(effect.command);
        assert_eq!(command.unwrap().name, "paste");

        // A stale seq (React remounted with a lower counter) is ignored rather
        // than replaying the whole command history.
        let (_, command) = props.set("command", &json!({ "seq": 1, "name": "copy" }));
        assert!(command.is_none());
    }

    #[test]
    fn a_malformed_command_is_ignored() {
        let mut props = GridProps::default();
        assert!(props.set("command", &json!({ "name": "copy" })).1.is_none());
        assert!(props.set("command", &json!("copy")).1.is_none());
        assert_eq!(props.command_seq, 0);
    }

    #[test]
    fn the_font_cache_key_changes_with_every_metric() {
        let mut props = GridProps::default();
        let base = props.font_key();
        props.set("fontSize", &json!(20));
        assert_ne!(base, props.font_key());
        let sized = props.font_key();
        props.set("fontFamily", &json!("Fira Code"));
        assert_ne!(sized, props.font_key());
        let familied = props.font_key();
        props.set("lineHeight", &json!(1.5));
        assert_ne!(familied, props.font_key());
    }

    #[test]
    fn react_can_drive_focus_through_the_prop() {
        let mut props = GridProps::default();
        assert_eq!(props.focused, None);
        let (effect, _) = props.set("focused", &json!(true));
        assert!(effect.focus);
        assert_eq!(props.focused, Some(true));
        let (effect, _) = props.set("focused", &json!(true));
        assert!(effect.is_noop());
        let (effect, _) = props.set("focused", &json!(false));
        assert!(effect.focus);
        assert_eq!(props.focused, Some(false));
    }

    #[test]
    fn theme_and_cursor_props_land() {
        let mut props = GridProps::default();
        let (effect, _) = props.set("theme", &json!({ "bg": "#101010" }));
        assert!(effect.theme);
        assert_eq!(props.palette.bg, (0x10, 0x10, 0x10));

        props.set("cursorStyle", &json!("beam"));
        assert_eq!(props.cursor_style, CursorStyle::Beam);
        props.set("cursorStyle", &json!("nonsense"));
        assert_eq!(props.cursor_style, CursorStyle::Block);
        props.set("cursorBlink", &json!(false));
        assert!(!props.cursor_blink);
    }
}
