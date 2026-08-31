//! PTY ownership for one Surface (`docs/plan/03-server.md` §4 and §9).
//!
//! Built on `portable-pty` 0.9. This module is *blocking* on purpose: the
//! Server puts the reader, the writer and `Child::wait` on their own threads
//! (`01-architecture.md`), and `st-core` never picks a runtime.
//!
//! What it owns:
//!
//! * shell resolution — `[shell].program` → `$SHELL` → the passwd entry
//!   (`CommandBuilder::get_shell`) → `/bin/sh`;
//! * the login-shell rule — `-l` for `bash`/`zsh`/`fish`, never for
//!   `sh`/`dash`;
//! * the environment: `TERM=xterm-256color`, `COLORTERM=truecolor`,
//!   `TERM_PROGRAM=superterminal`, `TERM_PROGRAM_VERSION`,
//!   `SUPERTERMINAL_SURFACE_ID`, `SUPERTERMINAL_SOCKET`, minus the multiplexer
//!   leftovers `TMUX`, `STY` and `TERM_SESSION_ID`;
//! * `set_controlling_tty(true)`, which gives the child its own session,
//!   process group and controlling terminal;
//! * `resize`, reader/writer handles, `wait` → exit status;
//! * the Q21 kill ladder: `killpg(pgid, SIGHUP)`, grace, `SIGKILL`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use st_proto::SurfaceId;

/// `TERM` handed to every child. Overriding it is refused (`§9`).
pub const TERM: &str = "xterm-256color";

/// The last-resort shell when nothing else resolves.
pub const FALLBACK_SHELL: &str = "/bin/sh";

/// Environment variables removed from every child (`§9`).
pub const STRIPPED_ENV: [&str; 3] = ["TMUX", "STY", "TERM_SESSION_ID"];

/// Anything that can go wrong opening a PTY or spawning into it.
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    /// `openpty` failed.
    #[error("openpty failed: {0}")]
    Open(String),
    /// `spawn_command` failed — usually a missing or non-executable shell.
    #[error("failed to spawn {program}: {source}")]
    Spawn {
        /// The program we tried to run.
        program: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// `resize` failed.
    #[error("pty resize failed: {0}")]
    Resize(String),
    /// A reader or writer handle could not be obtained.
    #[error("pty io handle unavailable: {0}")]
    Handle(String),
    /// Waiting on the child failed.
    #[error("waiting for the child failed: {0}")]
    Wait(#[source] std::io::Error),
}

/// How a Surface's process ended.
///
/// Richer than [`st_proto::ExitStatus`], which has no room for a signal
/// *name*; `portable-pty` reports the name, not the number.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExitStatus {
    /// Exit code, when the process exited normally.
    pub code: Option<i32>,
    /// Signal number, when it was killed and the name could be resolved.
    pub signal: Option<i32>,
    /// Signal name as the OS spelled it, e.g. `"Hangup"`.
    pub signal_name: Option<String>,
}

impl ExitStatus {
    /// `true` for a normal, zero exit.
    #[must_use]
    pub fn success(&self) -> bool {
        self.code == Some(0) && self.signal.is_none()
    }
}

impl From<portable_pty::ExitStatus> for ExitStatus {
    fn from(status: portable_pty::ExitStatus) -> Self {
        match status.signal() {
            Some(name) => Self {
                code: None,
                signal: signal_number(name),
                signal_name: Some(name.to_owned()),
            },
            None => Self {
                #[allow(clippy::cast_possible_wrap)]
                code: Some(status.exit_code() as i32),
                signal: None,
                signal_name: None,
            },
        }
    }
}

impl From<ExitStatus> for st_proto::ExitStatus {
    fn from(status: ExitStatus) -> Self {
        Self {
            code: status.code,
            signal: status.signal,
        }
    }
}

/// Best-effort mapping of a `strsignal` name back onto its number.
///
/// Only the signals a shell realistically dies from; anything else keeps its
/// name and reports `signal: None` on the wire.
#[must_use]
pub fn signal_number(name: &str) -> Option<i32> {
    let lowered = name.to_ascii_lowercase();
    let key = lowered.trim();
    Some(match key {
        "hangup" | "sighup" => 1,
        "interrupt" | "sigint" => 2,
        "quit" | "sigquit" => 3,
        "illegal instruction" | "sigill" => 4,
        "trace/breakpoint trap" | "sigtrap" => 5,
        "aborted" | "sigabrt" => 6,
        "bus error" | "sigbus" => 7,
        "floating point exception" | "sigfpe" => 8,
        "killed" | "sigkill" => 9,
        "user defined signal 1" | "sigusr1" => 10,
        "segmentation fault" | "sigsegv" => 11,
        "user defined signal 2" | "sigusr2" => 12,
        "broken pipe" | "sigpipe" => 13,
        "alarm clock" | "sigalrm" => 14,
        "terminated" | "sigterm" => 15,
        _ => return None,
    })
}

/// Everything needed to open a PTY and spawn a shell into it.
#[derive(Debug, Clone)]
pub struct PtyConfig {
    /// The Surface this PTY belongs to; exported as
    /// `SUPERTERMINAL_SURFACE_ID`.
    pub surface_id: SurfaceId,
    /// Grid width.
    pub cols: u16,
    /// Grid height.
    pub rows: u16,
    /// `[shell].program`; `None` falls back to `$SHELL`, then the passwd
    /// entry, then [`FALLBACK_SHELL`].
    pub program: Option<PathBuf>,
    /// Extra arguments after the login flag.
    pub args: Vec<String>,
    /// Append `-l` when the shell understands it.
    pub login: bool,
    /// Working directory; `None` means "inherit the Server's".
    pub cwd: Option<PathBuf>,
    /// Extra variables merged over the inherited environment.
    pub env: Vec<(String, String)>,
    /// The Server build id, exported as `TERM_PROGRAM_VERSION`.
    pub build_id: String,
    /// The Server's socket path, exported as `SUPERTERMINAL_SOCKET`.
    pub socket_path: Option<PathBuf>,
}

impl Default for PtyConfig {
    fn default() -> Self {
        Self {
            surface_id: SurfaceId::ZERO,
            cols: 80,
            rows: 24,
            program: None,
            args: Vec::new(),
            login: cfg!(target_os = "macos"),
            cwd: None,
            env: Vec::new(),
            build_id: String::new(),
            socket_path: None,
        }
    }
}

/// Resolves the shell to run: config, then `$SHELL`, then the passwd entry,
/// then [`FALLBACK_SHELL`] (`§9`).
#[must_use]
pub fn resolve_shell(configured: Option<&Path>) -> PathBuf {
    if let Some(program) = configured {
        return program.to_path_buf();
    }
    if let Some(shell) = std::env::var_os("SHELL") {
        if !shell.is_empty() {
            return PathBuf::from(shell);
        }
    }
    // `CommandBuilder::get_shell` consults the passwd entry and already falls
    // back to /bin/sh on its own.
    let shell = CommandBuilder::new_default_prog().get_shell();
    if shell.is_empty() {
        PathBuf::from(FALLBACK_SHELL)
    } else {
        PathBuf::from(shell)
    }
}

/// `true` when appending `-l` to this shell makes sense (`§9`).
#[must_use]
pub fn accepts_login_flag(shell: &Path) -> bool {
    matches!(
        shell.file_name().and_then(|n| n.to_str()),
        Some("bash" | "zsh" | "fish")
    )
}

/// Builds the `CommandBuilder` a [`PtyConfig`] describes, without spawning.
///
/// Exposed so the environment rules can be tested without a PTY.
#[must_use]
pub fn build_command(config: &PtyConfig) -> (PathBuf, CommandBuilder) {
    let shell = resolve_shell(config.program.as_deref());
    let mut cmd = CommandBuilder::new(&shell);
    if config.login && accepts_login_flag(&shell) {
        cmd.arg("-l");
    }
    cmd.args(&config.args);

    cmd.env("TERM", TERM);
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "superterminal");
    if !config.build_id.is_empty() {
        cmd.env("TERM_PROGRAM_VERSION", &config.build_id);
    }
    cmd.env(
        "SUPERTERMINAL_SURFACE_ID",
        config.surface_id.get().to_string(),
    );
    if let Some(socket) = &config.socket_path {
        cmd.env("SUPERTERMINAL_SOCKET", socket);
    }
    for name in STRIPPED_ENV {
        cmd.env_remove(name);
    }
    for (key, value) in &config.env {
        if key == "TERM" {
            tracing::warn!(%key, "refusing to override TERM for a Surface");
            continue;
        }
        cmd.env(key, value);
    }
    if let Some(cwd) = &config.cwd {
        cmd.cwd(cwd);
    }
    // The default is already true; being explicit documents that the child
    // gets its own session, process group and controlling tty (§9).
    cmd.set_controlling_tty(true);
    (shell, cmd)
}

/// A live PTY with a child process in it.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    shell: PathBuf,
    pid: Option<u32>,
    size: PtySize,
}

impl std::fmt::Debug for Pty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pty")
            .field("shell", &self.shell)
            .field("pid", &self.pid)
            .field("cols", &self.size.cols)
            .field("rows", &self.size.rows)
            .finish()
    }
}

impl Pty {
    /// Opens a PTY and spawns the configured shell into it.
    pub fn spawn(config: &PtyConfig) -> Result<Self, PtyError> {
        let size = PtySize {
            rows: config.rows.max(1),
            cols: config.cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|e| PtyError::Open(e.to_string()))?;
        let (shell, cmd) = build_command(config);
        let child = pair.slave.spawn_command(cmd).map_err(|e| PtyError::Spawn {
            program: shell.display().to_string(),
            source: std::io::Error::other(e.to_string()),
        })?;
        // Drop the slave right away: while the Server holds it open the master
        // never sees EOF after the child exits.
        drop(pair.slave);

        let killer = child.clone_killer();
        let pid = child.process_id();
        Ok(Self {
            master: pair.master,
            child,
            killer,
            shell,
            pid,
            size,
        })
    }

    /// The shell that was resolved and spawned.
    #[must_use]
    pub fn shell(&self) -> &Path {
        &self.shell
    }

    /// The child's pid, when the platform reports one.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// The current grid size as the kernel knows it.
    #[must_use]
    pub fn size(&self) -> (u16, u16) {
        (self.size.cols, self.size.rows)
    }

    /// A blocking reader over the child's output. Safe to move to a thread.
    pub fn reader(&self) -> Result<Box<dyn Read + Send>, PtyError> {
        self.master
            .try_clone_reader()
            .map_err(|e| PtyError::Handle(e.to_string()))
    }

    /// The writer for `Input` and VT replies. Valid to take exactly once.
    pub fn writer(&self) -> Result<Box<dyn Write + Send>, PtyError> {
        self.master
            .take_writer()
            .map_err(|e| PtyError::Handle(e.to_string()))
    }

    /// Sets the window size, which makes the kernel raise `SIGWINCH`.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PtyError> {
        let size = PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        self.master
            .resize(size)
            .map_err(|e| PtyError::Resize(e.to_string()))?;
        self.size = size;
        Ok(())
    }

    /// The foreground process group of the terminal (`tcgetpgrp`), used by the
    /// cwd probe and by the "has a foreground child" check (grilling Q48).
    #[must_use]
    pub fn foreground_pgid(&self) -> Option<u32> {
        #[cfg(unix)]
        {
            self.master
                .process_group_leader()
                .and_then(|pid| u32::try_from(pid).ok())
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Polls the child without blocking.
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, PtyError> {
        self.child
            .try_wait()
            .map_err(PtyError::Wait)
            .map(|status| status.map(Into::into))
    }

    /// Blocks until the child exits. Intended for the dedicated waiter thread.
    pub fn wait(&mut self) -> Result<ExitStatus, PtyError> {
        self.child.wait().map_err(PtyError::Wait).map(Into::into)
    }

    /// Sends `SIGHUP` to the child's whole process group (`§9`, Q21).
    ///
    /// Returns `false` when the platform or the child cannot be signalled;
    /// the caller should then fall back to [`Pty::kill`].
    pub fn hangup(&self) -> bool {
        self.signal_group(libc_sighup())
    }

    /// Sends `SIGKILL` to the child's process group, then to the child.
    pub fn kill(&mut self) {
        if !self.signal_group(libc_sigkill()) {
            let _ = self.killer.kill();
        }
    }

    /// A killer handle that can be moved to another thread while this one is
    /// blocked in [`Pty::wait`].
    #[must_use]
    pub fn killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        self.killer.clone_killer()
    }

    #[cfg(unix)]
    fn signal_group(&self, signal: i32) -> bool {
        let Some(pid) = self.pid else {
            return false;
        };
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: `killpg` takes two integers and touches no memory. A stale
        // pgid can only ever return ESRCH, which we report as `false`.
        #[allow(unsafe_code)]
        let rc = unsafe { libc::killpg(pid, signal) };
        rc == 0
    }

    #[cfg(not(unix))]
    fn signal_group(&self, _signal: i32) -> bool {
        false
    }
}

#[cfg(unix)]
const fn libc_sighup() -> i32 {
    libc::SIGHUP
}
#[cfg(not(unix))]
const fn libc_sighup() -> i32 {
    1
}
#[cfg(unix)]
const fn libc_sigkill() -> i32 {
    libc::SIGKILL
}
#[cfg(not(unix))]
const fn libc_sigkill() -> i32 {
    9
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_flag_only_for_shells_that_understand_it() {
        assert!(accepts_login_flag(Path::new("/bin/bash")));
        assert!(accepts_login_flag(Path::new("/usr/bin/zsh")));
        assert!(accepts_login_flag(Path::new("/usr/local/bin/fish")));
        assert!(!accepts_login_flag(Path::new("/bin/sh")));
        assert!(!accepts_login_flag(Path::new("/bin/dash")));
    }

    #[test]
    fn configured_shell_wins() {
        assert_eq!(
            resolve_shell(Some(Path::new("/bin/dash"))),
            PathBuf::from("/bin/dash")
        );
        // Whatever the environment says, the result is always absolute-ish and
        // non-empty.
        assert!(!resolve_shell(None).as_os_str().is_empty());
    }

    #[test]
    fn environment_follows_the_spec() {
        let config = PtyConfig {
            surface_id: SurfaceId::new(42),
            program: Some(PathBuf::from("/bin/bash")),
            login: true,
            build_id: "cafe1234".into(),
            socket_path: Some(PathBuf::from("/run/st/sock")),
            env: vec![
                ("LANG".into(), "en_GB.UTF-8".into()),
                ("TERM".into(), "dumb".into()),
            ],
            ..PtyConfig::default()
        };
        let (shell, cmd) = build_command(&config);
        assert_eq!(shell, PathBuf::from("/bin/bash"));
        assert_eq!(cmd.get_argv()[1], "-l");

        let env: std::collections::HashMap<_, _> = cmd.iter_extra_env_as_str().collect();
        assert_eq!(env.get("TERM"), Some(&TERM), "TERM override is refused");
        assert_eq!(env.get("COLORTERM"), Some(&"truecolor"));
        assert_eq!(env.get("TERM_PROGRAM"), Some(&"superterminal"));
        assert_eq!(env.get("TERM_PROGRAM_VERSION"), Some(&"cafe1234"));
        assert_eq!(env.get("SUPERTERMINAL_SURFACE_ID"), Some(&"42"));
        assert_eq!(env.get("SUPERTERMINAL_SOCKET"), Some(&"/run/st/sock"));
        assert_eq!(env.get("LANG"), Some(&"en_GB.UTF-8"));
    }

    #[test]
    fn no_login_flag_for_sh() {
        let config = PtyConfig {
            program: Some(PathBuf::from("/bin/sh")),
            login: true,
            args: vec!["-c".into(), "true".into()],
            ..PtyConfig::default()
        };
        let (_, cmd) = build_command(&config);
        let argv: Vec<_> = cmd
            .get_argv()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, vec!["/bin/sh", "-c", "true"]);
    }

    #[test]
    fn signal_names_map_back_to_numbers() {
        assert_eq!(signal_number("Hangup"), Some(1));
        assert_eq!(signal_number("Killed"), Some(9));
        assert_eq!(signal_number("Terminated"), Some(15));
        assert_eq!(signal_number("Whatever"), None);
    }

    #[test]
    fn exit_status_maps_onto_the_wire_type() {
        let normal: ExitStatus = portable_pty::ExitStatus::with_exit_code(3).into();
        assert_eq!(normal.code, Some(3));
        assert!(!normal.success());
        let wire: st_proto::ExitStatus = normal.into();
        assert_eq!(wire.code, Some(3));
        assert_eq!(wire.signal, None);

        let killed: ExitStatus = portable_pty::ExitStatus::with_signal("Hangup").into();
        assert_eq!(killed.signal, Some(1));
        assert_eq!(killed.signal_name.as_deref(), Some("Hangup"));
        assert_eq!(killed.code, None);
    }
}
