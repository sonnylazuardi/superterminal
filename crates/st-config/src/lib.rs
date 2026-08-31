#![deny(missing_docs)]
//! Shared `config.toml` schema, loader and path resolution for superterminal.
//!
//! One file, `config.toml`, configures both processes (Q34): `superterminald`
//! reads `[shell]`, `[server]` and `[theme]`; the client reads `[font]`,
//! `[window]`, `[terminal]`, `[theme]` and `[keybindings]`. This crate is the
//! Rust half of that schema — the Bun app parses the very same file with
//! `Bun.TOML.parse` and a zod schema (Q46), and `docs/config-example.toml`,
//! generated from this crate, is the shared fixture both sides validate
//! against.
//!
//! ```no_run
//! let loaded = st_config::Config::load_verbose()?;
//! for w in &loaded.warnings {
//!     eprintln!("warning: {w}");
//! }
//! let shell = loaded.config.resolve_shell();
//! println!("{} {:?} in {}", shell.program.display(), shell.args, st_config::runtime_dir().display());
//! # Ok::<(), st_config::ConfigError>(())
//! ```
//!
//! It has no dependency on tokio, GPUI or any protocol crate: only `serde`,
//! `toml` and `thiserror`.

mod color;
mod commands;
mod error;
mod example;
mod num;
mod paths;
mod sections;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use crate::color::{ParseColorError, Rgb};
pub use crate::commands::{is_known_command, validate_shortcut, COMMAND_IDS, MODIFIERS};
pub use crate::error::ConfigError;
pub use crate::paths::{
    cache_dir, config_dir, config_path, current_uid, ensure_cache_dir, ensure_config_dir,
    ensure_log_dir, ensure_runtime_dir, ensure_socket_path, ensure_state_dir, lock_path, log_dir,
    runtime_dir, socket_path, state_dir, workspace_file, Paths, Platform, APP_DIR,
    CONFIG_FILE_NAME, DIR_MODE, LOCK_FILE_NAME, SOCKET_FILE_NAME,
};
pub use crate::sections::{
    BackspaceSends, FontConfig, Keybindings, OptionAsAlt, Padding, ResolvedShell, ServerConfig,
    ShellConfig, TerminalConfig, ThemeConfig, WindowBackground, WindowConfig,
};

/// The whole of `config.toml`.
///
/// Deserialising is total: every section and every key is optional, so an
/// empty file, a missing file and `Config::default()` all produce the same
/// value.
///
/// ```
/// let cfg: st_config::Config = st_config::Config::parse_str("[font]\nsize = 15.0\n").unwrap();
/// assert_eq!(cfg.font.size, 15.0);
/// assert_eq!(cfg.font.line_height, 1.2); // untouched default
/// ```
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `[font]`
    pub font: FontConfig,
    /// `[window]`
    pub window: WindowConfig,
    /// `[shell]`
    pub shell: ShellConfig,
    /// `[terminal]`
    pub terminal: TerminalConfig,
    /// `[theme]`
    pub theme: ThemeConfig,
    /// `[keybindings]`
    pub keybindings: Keybindings,
    /// `[server]`
    pub server: ServerConfig,
}

/// The result of a load: the configuration plus everything non-fatal we found.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The configuration, with defaults filled in and out-of-range values clamped.
    pub config: Config,
    /// The file we looked at.
    pub path: PathBuf,
    /// Whether that file existed. When `false`, `config` is [`Config::default`].
    pub found: bool,
    /// Human-readable notes: unknown keys, clamped values, unknown command ids.
    /// Callers should log these at `warn`.
    pub warnings: Vec<String>,
}

impl Config {
    /// The path [`Config::load`] reads, honouring `$SUPERTERMINAL_CONFIG`,
    /// `$XDG_CONFIG_HOME` and the platform convention.
    ///
    /// See [`Paths`] for the full table and for a version that does not read
    /// the process environment.
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        Paths::from_env().config_path()
    }

    /// Loads [`Config::default_path`], returning defaults when it does not exist.
    ///
    /// Warnings are discarded; use [`Config::load_verbose`] to see them.
    pub fn load() -> Result<Self, ConfigError> {
        Ok(Self::load_verbose()?.config)
    }

    /// Loads [`Config::default_path`] and reports what it found.
    pub fn load_verbose() -> Result<Loaded, ConfigError> {
        Self::load_from_verbose(Self::default_path()?)
    }

    /// Loads a specific file, returning defaults when it does not exist.
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        Ok(Self::load_from_verbose(path)?.config)
    }

    /// Loads a specific file and reports what it found.
    ///
    /// A missing file is not an error (Q34: "a missing file yields defaults").
    /// Anything else — an unreadable file, invalid TOML, a bad colour, an
    /// unknown enum variant — is a [`ConfigError`] carrying the line and column.
    pub fn load_from_verbose(path: impl AsRef<Path>) -> Result<Loaded, ConfigError> {
        let path = path.as_ref().to_path_buf();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Loaded {
                    config: Self::default(),
                    path,
                    found: false,
                    warnings: Vec::new(),
                });
            }
            Err(source) => return Err(ConfigError::Read { path, source }),
        };

        let mut loaded = Self::parse_str_verbose_with_origin(&text, &path.display().to_string())?;
        loaded.path = path;
        loaded.found = true;
        Ok(loaded)
    }

    /// Parses TOML text, ignoring warnings.
    pub fn parse_str(text: &str) -> Result<Self, ConfigError> {
        Ok(Self::parse_str_verbose(text)?.config)
    }

    /// Parses TOML text and reports warnings. `path` is reported as `<string>`.
    pub fn parse_str_verbose(text: &str) -> Result<Loaded, ConfigError> {
        Self::parse_str_verbose_with_origin(text, "<string>")
    }

    fn parse_str_verbose_with_origin(text: &str, origin: &str) -> Result<Loaded, ConfigError> {
        let mut config: Self = toml::from_str(text)
            .map_err(|source| ConfigError::parse(origin.to_owned(), text, source))?;

        // Re-parse untyped so unknown keys can be reported. This cannot fail:
        // the typed parse above already accepted the text.
        let raw: toml::Table = toml::from_str(text)
            .map_err(|source| ConfigError::parse(origin.to_owned(), text, source))?;

        let mut warnings = unknown_key_warnings(&raw, &config)?;
        warnings.extend(config.normalize());
        warnings.extend(config.keybinding_warnings());

        Ok(Loaded {
            config,
            path: PathBuf::from(origin),
            found: true,
            warnings,
        })
    }

    /// Clamps out-of-range values to something usable, returning one warning
    /// per adjustment. Called automatically by every loader.
    pub fn normalize(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();

        if !(self.font.size.is_finite() && self.font.size > 0.0) {
            warnings.push(format!(
                "font.size = {} is not a positive number; using {}",
                self.font.size,
                FontConfig::default().size
            ));
            self.font.size = FontConfig::default().size;
        }
        if !(self.font.line_height.is_finite() && self.font.line_height > 0.0) {
            warnings.push(format!(
                "font.line_height = {} is not a positive number; using {}",
                self.font.line_height,
                FontConfig::default().line_height
            ));
            self.font.line_height = FontConfig::default().line_height;
        }
        for (name, value) in [
            ("top", &mut self.window.padding.top),
            ("right", &mut self.window.padding.right),
            ("bottom", &mut self.window.padding.bottom),
            ("left", &mut self.window.padding.left),
        ] {
            if !(value.is_finite() && *value >= 0.0) {
                warnings.push(format!(
                    "window.padding.{name} = {value} is not a non-negative number; using 0"
                ));
                *value = 0.0;
            }
        }
        if self.terminal.scrollback_lines > TerminalConfig::MAX_SCROLLBACK_LINES {
            warnings.push(format!(
                "terminal.scrollback_lines = {} exceeds the maximum of {}; clamped",
                self.terminal.scrollback_lines,
                TerminalConfig::MAX_SCROLLBACK_LINES
            ));
            self.terminal.scrollback_lines = TerminalConfig::MAX_SCROLLBACK_LINES;
        }
        if !(self.server.idle_exit_minutes.is_finite() && self.server.idle_exit_minutes >= 0.0) {
            warnings.push(format!(
                "server.idle_exit_minutes = {} is not a non-negative number; using {}",
                self.server.idle_exit_minutes,
                ServerConfig::default().idle_exit_minutes
            ));
            self.server.idle_exit_minutes = ServerConfig::default().idle_exit_minutes;
        }

        warnings
    }

    /// Warnings about `[keybindings]` entries: unknown command ids and
    /// malformed shortcut strings. Both are non-fatal.
    pub fn keybinding_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        for (id, shortcut) in &self.keybindings {
            if !is_known_command(id) {
                warnings.push(format!("keybindings: unknown command id `{id}`; ignored"));
            }
            if let Err(reason) = validate_shortcut(shortcut) {
                warnings.push(format!(
                    "keybindings.{id}: invalid shortcut {shortcut:?}: {reason}"
                ));
            }
        }
        warnings
    }

    /// The commented `config.toml` that `st config init` writes.
    ///
    /// Parsing the result yields a value equal to `self`, so the generated
    /// document is a valid configuration and not just documentation.
    pub fn to_toml_string(&self) -> Result<String, ConfigError> {
        example::render(self)
    }

    /// The same values without comments, for machine consumers.
    pub fn to_toml_string_compact(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Writes [`Config::to_toml_string`] to `path`, creating the parent
    /// directory with mode `0700` if needed.
    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(path, self.to_toml_string()?).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Resolves the shell command line: `[shell].program` → `$SHELL` → `/bin/sh`.
    ///
    /// Reads `$SHELL` from the process environment; use
    /// [`ShellConfig::resolve`] to pass it explicitly.
    pub fn resolve_shell(&self) -> ResolvedShell {
        let shell_env = std::env::var("SHELL").ok();
        self.shell.resolve(shell_env.as_deref())
    }
}

/// Reports keys present in the parsed file but absent from the round-tripped
/// configuration, i.e. keys the schema does not know.
fn unknown_key_warnings(raw: &toml::Table, config: &Config) -> Result<Vec<String>, ConfigError> {
    let known = toml::Table::try_from(config)?;

    let mut raw_paths = BTreeSet::new();
    leaf_paths(&toml::Value::Table(raw.clone()), "", &mut raw_paths);
    let mut known_paths = BTreeSet::new();
    leaf_paths(&toml::Value::Table(known), "", &mut known_paths);

    Ok(raw_paths
        .difference(&known_paths)
        .map(|p| format!("unknown key `{p}`; ignored"))
        .collect())
}

/// Collects the dotted path of every non-table value in `value`.
fn leaf_paths(value: &toml::Value, prefix: &str, out: &mut BTreeSet<String>) {
    match value {
        toml::Value::Table(t) => {
            for (k, v) in t {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                leaf_paths(v, &path, out);
            }
        }
        _ => {
            out.insert(prefix.to_owned());
        }
    }
}
