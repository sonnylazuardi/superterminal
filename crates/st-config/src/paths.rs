//! Directory and file locations shared by `superterminald`, `st` and the app.
//!
//! Every location is derived from environment variables so that a test (or a
//! second development instance) can relocate the whole installation by
//! exporting a handful of variables. The free functions at the bottom of this
//! module read the real process environment; [`Paths`] can be built from an
//! arbitrary lookup so tests never have to touch `$HOME`.
//!
//! | What | Linux | macOS |
//! |---|---|---|
//! | config | `$XDG_CONFIG_HOME/superterminal` → `~/.config/superterminal` | `$XDG_CONFIG_HOME/superterminal` → `~/Library/Application Support/superterminal` |
//! | runtime | `$XDG_RUNTIME_DIR/superterminal` → `$TMPDIR/superterminal-$UID` → `/tmp/superterminal-$UID` | same |
//! | state | `$XDG_STATE_HOME/superterminal` → `~/.local/state/superterminal` | `~/Library/Application Support/superterminal` |
//! | cache | `$XDG_CACHE_HOME/superterminal` → `~/.cache/superterminal` | `~/Library/Caches/superterminal` |
//! | logs | `<state>/logs` | `<state>/logs` |
//!
//! Overrides, checked first in every case: `$SUPERTERMINAL_CONFIG` (a *file*),
//! `$SUPERTERMINAL_RUNTIME_DIR`, `$SUPERTERMINAL_SOCKET` (a *file*),
//! `$SUPERTERMINAL_STATE_DIR`, `$SUPERTERMINAL_CACHE_DIR`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::ConfigError;

/// The application directory name used under every base directory.
pub const APP_DIR: &str = "superterminal";

/// The file name of the configuration file.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// The file name of the server's Unix socket inside the runtime directory.
///
/// There is a single socket; a data-plane connection announces itself with the
/// 4-byte magic `0xFF S T D` (Q37), a control connection starts with `{`.
pub const SOCKET_FILE_NAME: &str = "server.sock";

/// The file name of the server's `flock` lock file inside the runtime directory.
pub const LOCK_FILE_NAME: &str = "lock";

/// Mode applied to every directory this crate creates.
pub const DIR_MODE: u32 = 0o700;

/// Which platform's directory conventions to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    /// XDG base directories.
    Linux,
    /// Apple's `~/Library` layout (with `$XDG_CONFIG_HOME` still honoured when set).
    MacOs,
}

impl Platform {
    /// The platform this binary was compiled for.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

impl Default for Platform {
    fn default() -> Self {
        Self::current()
    }
}

/// A resolved set of superterminal locations.
///
/// Construct with [`Paths::from_env`] in production, or [`Paths::from_lookup`]
/// in tests.
#[derive(Debug, Clone)]
pub struct Paths {
    platform: Platform,
    uid: u32,
    home: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
    xdg_state_home: Option<PathBuf>,
    xdg_cache_home: Option<PathBuf>,
    xdg_runtime_dir: Option<PathBuf>,
    tmpdir: Option<PathBuf>,
    config_override: Option<PathBuf>,
    runtime_override: Option<PathBuf>,
    socket_override: Option<PathBuf>,
    state_override: Option<PathBuf>,
    cache_override: Option<PathBuf>,
}

/// Reads an environment variable, treating the empty string as unset (as the
/// XDG specification requires).
fn non_empty(value: Option<OsString>) -> Option<PathBuf> {
    let value = value?;
    if value.is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
}

impl Paths {
    /// Resolves paths from the real process environment and the current uid.
    pub fn from_env() -> Self {
        Self::from_lookup(Platform::current(), current_uid(), |key| {
            std::env::var_os(key)
        })
    }

    /// Resolves paths from an arbitrary environment.
    ///
    /// `lookup` is called with the bare variable names (`"HOME"`,
    /// `"XDG_CONFIG_HOME"`, `"SUPERTERMINAL_CONFIG"`, …).
    pub fn from_lookup<F>(platform: Platform, uid: u32, lookup: F) -> Self
    where
        F: Fn(&str) -> Option<OsString>,
    {
        let get = |key: &str| non_empty(lookup(key));
        Self {
            platform,
            uid,
            home: get("HOME"),
            xdg_config_home: get("XDG_CONFIG_HOME"),
            xdg_state_home: get("XDG_STATE_HOME"),
            xdg_cache_home: get("XDG_CACHE_HOME"),
            xdg_runtime_dir: get("XDG_RUNTIME_DIR"),
            tmpdir: get("TMPDIR"),
            config_override: get("SUPERTERMINAL_CONFIG"),
            runtime_override: get("SUPERTERMINAL_RUNTIME_DIR"),
            socket_override: get("SUPERTERMINAL_SOCKET"),
            state_override: get("SUPERTERMINAL_STATE_DIR"),
            cache_override: get("SUPERTERMINAL_CACHE_DIR"),
        }
    }

    /// The platform conventions in use.
    pub fn platform(&self) -> Platform {
        self.platform
    }

    fn home(&self, what: &'static str) -> Result<&Path, ConfigError> {
        self.home.as_deref().ok_or(ConfigError::NoHomeDir { what })
    }

    /// The directory holding `config.toml`.
    ///
    /// `$XDG_CONFIG_HOME` wins on both platforms when it is set; otherwise
    /// Linux uses `~/.config` and macOS `~/Library/Application Support`.
    pub fn config_dir(&self) -> Result<PathBuf, ConfigError> {
        if let Some(base) = &self.xdg_config_home {
            return Ok(base.join(APP_DIR));
        }
        let home = self.home.as_deref().ok_or(ConfigError::NoConfigDir)?;
        Ok(match self.platform {
            Platform::Linux => home.join(".config").join(APP_DIR),
            Platform::MacOs => home
                .join("Library")
                .join("Application Support")
                .join(APP_DIR),
        })
    }

    /// The configuration file itself.
    ///
    /// `$SUPERTERMINAL_CONFIG` overrides it completely and is used verbatim
    /// (it names a file, not a directory).
    pub fn config_path(&self) -> Result<PathBuf, ConfigError> {
        if let Some(path) = &self.config_override {
            return Ok(path.clone());
        }
        Ok(self.config_dir()?.join(CONFIG_FILE_NAME))
    }

    /// The runtime directory holding the socket and the lock file.
    ///
    /// `$SUPERTERMINAL_RUNTIME_DIR` → `$XDG_RUNTIME_DIR/superterminal` →
    /// `$TMPDIR/superterminal-$UID` → `/tmp/superterminal-$UID`. Never fails:
    /// the last fallback needs no environment at all.
    pub fn runtime_dir(&self) -> PathBuf {
        if let Some(dir) = &self.runtime_override {
            return dir.clone();
        }
        if let Some(dir) = &self.xdg_runtime_dir {
            return dir.join(APP_DIR);
        }
        let tmp = self.tmpdir.clone().unwrap_or_else(|| PathBuf::from("/tmp"));
        tmp.join(format!("{APP_DIR}-{}", self.uid))
    }

    /// The server's Unix socket. `$SUPERTERMINAL_SOCKET` overrides it verbatim.
    pub fn socket_path(&self) -> PathBuf {
        self.socket_override
            .clone()
            .unwrap_or_else(|| self.runtime_dir().join(SOCKET_FILE_NAME))
    }

    /// The server's `flock` lock file, always beside the socket's default
    /// location (never moved by `$SUPERTERMINAL_SOCKET`).
    pub fn lock_path(&self) -> PathBuf {
        self.runtime_dir().join(LOCK_FILE_NAME)
    }

    /// The state directory holding `workspace.json`, `logs/` and recordings.
    pub fn state_dir(&self) -> Result<PathBuf, ConfigError> {
        if let Some(dir) = &self.state_override {
            return Ok(dir.clone());
        }
        if let Some(base) = &self.xdg_state_home {
            return Ok(base.join(APP_DIR));
        }
        let home = self.home("state")?;
        Ok(match self.platform {
            Platform::Linux => home.join(".local").join("state").join(APP_DIR),
            Platform::MacOs => home
                .join("Library")
                .join("Application Support")
                .join(APP_DIR),
        })
    }

    /// The cache directory (shaped-glyph caches, downloaded assets, …).
    pub fn cache_dir(&self) -> Result<PathBuf, ConfigError> {
        if let Some(dir) = &self.cache_override {
            return Ok(dir.clone());
        }
        if let Some(base) = &self.xdg_cache_home {
            return Ok(base.join(APP_DIR));
        }
        let home = self.home("cache")?;
        Ok(match self.platform {
            Platform::Linux => home.join(".cache").join(APP_DIR),
            Platform::MacOs => home.join("Library").join("Caches").join(APP_DIR),
        })
    }

    /// The log directory: `logs/` inside [`Paths::state_dir`].
    pub fn log_dir(&self) -> Result<PathBuf, ConfigError> {
        Ok(self.state_dir()?.join("logs"))
    }

    /// The workspace document written by the server.
    pub fn workspace_file(&self) -> Result<PathBuf, ConfigError> {
        Ok(self.state_dir()?.join("workspace.json"))
    }

    /// Creates [`Paths::config_dir`] with mode `0700` and returns it.
    pub fn ensure_config_dir(&self) -> Result<PathBuf, ConfigError> {
        create_dir_700(self.config_dir()?)
    }

    /// Creates [`Paths::runtime_dir`] with mode `0700` and returns it.
    pub fn ensure_runtime_dir(&self) -> Result<PathBuf, ConfigError> {
        create_dir_700(self.runtime_dir())
    }

    /// Creates [`Paths::state_dir`] with mode `0700` and returns it.
    pub fn ensure_state_dir(&self) -> Result<PathBuf, ConfigError> {
        create_dir_700(self.state_dir()?)
    }

    /// Creates [`Paths::cache_dir`] with mode `0700` and returns it.
    pub fn ensure_cache_dir(&self) -> Result<PathBuf, ConfigError> {
        create_dir_700(self.cache_dir()?)
    }

    /// Creates [`Paths::log_dir`] (and its parents) with mode `0700`.
    pub fn ensure_log_dir(&self) -> Result<PathBuf, ConfigError> {
        create_dir_700(self.log_dir()?)
    }

    /// Creates the runtime directory and returns the socket path inside it.
    pub fn ensure_socket_path(&self) -> Result<PathBuf, ConfigError> {
        let path = self.socket_path();
        if let Some(parent) = path.parent() {
            create_dir_700(parent.to_path_buf())?;
        }
        Ok(path)
    }
}

impl Default for Paths {
    fn default() -> Self {
        Self::from_env()
    }
}

/// Creates `dir` and every missing parent, then forces mode `0700` on `dir`.
///
/// Existing directories have their mode fixed too: the socket lives inside and
/// `03-server.md` §10 requires `0700`.
fn create_dir_700(dir: PathBuf) -> Result<PathBuf, ConfigError> {
    let map = |source| ConfigError::CreateDir {
        path: dir.clone(),
        source,
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(&dir)
            .map_err(map)?;
        let perms = std::fs::Permissions::from_mode(DIR_MODE);
        std::fs::set_permissions(&dir, perms).map_err(map)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(&dir).map_err(map)?;
    }

    Ok(dir)
}

/// Best-effort uid of the current user, used only to name the `/tmp` fallback
/// runtime directory.
///
/// Tries `/proc/self` (Linux), then `$HOME`, then a throw-away file in the
/// system temp directory; returns `0` if all of those fail. No `libc`
/// dependency, by design.
#[cfg(unix)]
pub fn current_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;

    if let Ok(md) = std::fs::metadata("/proc/self") {
        return md.uid();
    }
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        if let Ok(md) = std::fs::metadata(home) {
            return md.uid();
        }
    }
    let probe =
        std::env::temp_dir().join(format!(".superterminal-uid-probe-{}", std::process::id()));
    let uid = std::fs::write(&probe, b"")
        .and_then(|()| std::fs::metadata(&probe))
        .map(|md| md.uid())
        .unwrap_or(0);
    let _ = std::fs::remove_file(&probe);
    uid
}

/// Always `0` off Unix; the fallback runtime directory is never used there.
#[cfg(not(unix))]
pub fn current_uid() -> u32 {
    0
}

macro_rules! forward {
    ($(#[$m:meta])* $name:ident -> PathBuf) => {
        $(#[$m])*
        pub fn $name() -> PathBuf { Paths::from_env().$name() }
    };
    ($(#[$m:meta])* $name:ident -> Result) => {
        $(#[$m])*
        pub fn $name() -> Result<PathBuf, ConfigError> { Paths::from_env().$name() }
    };
}

forward!(
    /// [`Paths::config_dir`] for the current process environment.
    config_dir -> Result
);
forward!(
    /// [`Paths::config_path`] for the current process environment.
    config_path -> Result
);
forward!(
    /// [`Paths::runtime_dir`] for the current process environment.
    runtime_dir -> PathBuf
);
forward!(
    /// [`Paths::socket_path`] for the current process environment.
    socket_path -> PathBuf
);
forward!(
    /// [`Paths::lock_path`] for the current process environment.
    lock_path -> PathBuf
);
forward!(
    /// [`Paths::state_dir`] for the current process environment.
    state_dir -> Result
);
forward!(
    /// [`Paths::cache_dir`] for the current process environment.
    cache_dir -> Result
);
forward!(
    /// [`Paths::log_dir`] for the current process environment.
    log_dir -> Result
);
forward!(
    /// [`Paths::workspace_file`] for the current process environment.
    workspace_file -> Result
);
forward!(
    /// [`Paths::ensure_config_dir`] for the current process environment.
    ensure_config_dir -> Result
);
forward!(
    /// [`Paths::ensure_runtime_dir`] for the current process environment.
    ensure_runtime_dir -> Result
);
forward!(
    /// [`Paths::ensure_state_dir`] for the current process environment.
    ensure_state_dir -> Result
);
forward!(
    /// [`Paths::ensure_cache_dir`] for the current process environment.
    ensure_cache_dir -> Result
);
forward!(
    /// [`Paths::ensure_log_dir`] for the current process environment.
    ensure_log_dir -> Result
);
forward!(
    /// [`Paths::ensure_socket_path`] for the current process environment.
    ensure_socket_path -> Result
);
