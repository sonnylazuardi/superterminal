//! Process lifecycle — `docs/plan/03-server.md` §2.
//!
//! Startup, in order:
//!
//! 1. resolve the paths (`st-config`), creating the runtime directory `0700`;
//! 2. `flock(LOCK_EX | LOCK_NB)` the lock file and write our pid into it — a
//!    failure means another daemon is already serving this user, which is the
//!    normal outcome of the client's spawn race (grilling Q30);
//! 3. unlink a stale socket (safe *because* we hold the lock, so nobody can be
//!    serving it), bind, `chmod 0600`;
//! 4. load `workspace.json`, moving a corrupt one aside, and re-seed the saved
//!    shape with fresh Surfaces;
//! 5. start the actor, the accept loop, the idle timer and the signal handlers.
//!
//! Shutdown is one path, whatever triggers it (`SIGTERM`, `SIGINT`,
//! `server.shutdown`, idle exit): publish `ev.server_shutting_down`, flush
//! `workspace.json` bypassing the debounce, signal the Surfaces, unlink the
//! socket, release the lock.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use st_config::{Config, Paths, Platform};
use tokio::net::UnixListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::control::{self, DataAcceptor};
use crate::metrics::{Metrics, Uptime};
use crate::persist::{self, Persister};
use crate::supervisor::{DeferredNotifier, SupervisorConfig, SurfaceSupervisor, WorkspaceNotifier};
use crate::workspace::model::Workspace;
use crate::workspace::spawn::{NullSpawner, SurfaceSpawner};
use crate::workspace::{ActorConfig, SpawnDefaults, WorkspaceActor, WorkspaceHandle};
use crate::ServerContext;

/// Mode applied to the socket (§10).
pub const SOCKET_MODE: u32 = 0o600;

/// How long the daemon waits for shutdown notices to reach clients before it
/// closes their sockets.
pub const SHUTDOWN_GRACE: Duration = Duration::from_millis(50);

// ------------------------------------------------------------ shutdown

/// The single graceful-shutdown trigger.
///
/// Cloneable and idempotent: the first reason wins, later triggers are logged
/// and ignored, so `SIGTERM` during an idle exit cannot start two shutdowns.
#[derive(Debug, Clone)]
pub struct Shutdown {
    tx: watch::Sender<Option<String>>,
}

/// The receiving side of [`Shutdown`].
#[derive(Debug, Clone)]
pub struct ShutdownWatch {
    rx: watch::Receiver<Option<String>>,
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

impl Shutdown {
    /// A trigger that has not fired.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = watch::channel(None);
        Self { tx }
    }

    /// Fires the trigger, unless it already fired.
    pub fn trigger(&self, reason: impl Into<String>) {
        let reason = reason.into();
        self.tx.send_if_modified(|current| {
            if current.is_some() {
                tracing::debug!(reason, "shutdown already in progress");
                false
            } else {
                tracing::info!(reason, "shutting down");
                *current = Some(reason.clone());
                true
            }
        });
    }

    /// `true` once [`Shutdown::trigger`] has been called.
    #[must_use]
    pub fn is_triggered(&self) -> bool {
        self.tx.borrow().is_some()
    }

    /// The reason, once it fired.
    #[must_use]
    pub fn reason(&self) -> Option<String> {
        self.tx.borrow().clone()
    }

    /// A watch that [`ShutdownWatch::wait`] can be awaited on.
    #[must_use]
    pub fn subscribe(&self) -> ShutdownWatch {
        ShutdownWatch {
            rx: self.tx.subscribe(),
        }
    }
}

impl ShutdownWatch {
    /// Resolves as soon as the trigger has fired (immediately if it already
    /// has).
    pub async fn wait(&mut self) -> String {
        loop {
            if let Some(reason) = self.rx.borrow_and_update().clone() {
                return reason;
            }
            if self.rx.changed().await.is_err() {
                return "channel closed".to_string();
            }
        }
    }
}

// ------------------------------------------------------------ lock file

/// Why the daemon could not take the single-instance lock.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another `superterminald` holds the lock (§2, "Single instance").
    #[error("another superterminald is already running{}, holding {path}", pid_suffix(*pid))]
    AlreadyRunning {
        /// The pid found in the lock file, when it was readable.
        pid: Option<u32>,
        /// The lock file.
        path: PathBuf,
    },
    /// The lock file could not be opened or written.
    #[error("cannot use the lock file {path}: {source}")]
    Io {
        /// The lock file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

fn pid_suffix(pid: Option<u32>) -> String {
    pid.map_or_else(String::new, |pid| format!(" (pid {pid})"))
}

/// An exclusive `flock` held for the life of the process, with our pid in it.
///
/// Dropping it releases the lock (the kernel does so when the fd closes); the
/// file itself is left behind on purpose, because deleting it would race with
/// the next daemon's `open`.
#[derive(Debug)]
pub struct LockFile {
    // Held solely for its side effect: closing the fd releases the flock.
    _file: std::fs::File,
    path: PathBuf,
}

impl LockFile {
    /// Takes the lock, or reports who holds it.
    pub fn acquire(path: &Path) -> Result<Self, LockError> {
        use std::io::{Seek, Write};

        let io = |source| LockError::Io {
            path: path.to_path_buf(),
            source,
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(io)?;

        if let Err(e) =
            rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        {
            return Err(match e {
                rustix::io::Errno::WOULDBLOCK | rustix::io::Errno::INTR => {
                    LockError::AlreadyRunning {
                        pid: read_pid(path),
                        path: path.to_path_buf(),
                    }
                }
                other => LockError::Io {
                    path: path.to_path_buf(),
                    source: std::io::Error::from(other),
                },
            });
        }

        file.set_len(0).map_err(io)?;
        file.rewind().map_err(io)?;
        writeln!(file, "{}", std::process::id()).map_err(io)?;
        file.flush().map_err(io)?;

        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }

    /// The file being held.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

// ------------------------------------------------------------ logging

/// Keeps the non-blocking log writer alive; drop it last (§2, "Logging").
#[derive(Debug)]
pub struct LogGuard(#[allow(dead_code)] Option<tracing_appender::non_blocking::WorkerGuard>);

/// Installs `tracing`: a daily rolling file in `log_dir()`, plus stderr when
/// running in the foreground.
///
/// The filter comes from `$SUPERTERMINAL_LOG`, else from `-v`/`-vv`, else
/// `info`. Called once per process; a second call is a no-op that returns an
/// empty guard, which is what makes it safe from tests.
pub fn init_logging(paths: &Paths, foreground: bool, verbosity: u8) -> LogGuard {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;
    use tracing_subscriber::{fmt, EnvFilter};

    let default = match verbosity {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_env("SUPERTERMINAL_LOG").unwrap_or_else(|_| {
        EnvFilter::new(format!("st_server={default},superterminald={default},warn"))
    });

    let (file_layer, guard) = match paths.ensure_log_dir() {
        Ok(dir) => {
            let appender = tracing_appender::rolling::daily(dir, "superterminald.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            (
                Some(fmt::layer().with_ansi(false).with_writer(writer)),
                Some(guard),
            )
        }
        Err(e) => {
            eprintln!("superterminald: cannot open the log directory: {e}");
            (None, None)
        }
    };

    let stderr_layer = foreground.then(|| fmt::layer().with_writer(std::io::stderr));

    let installed = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .is_ok();

    if !installed {
        // Already initialised (another test in the same process, say).
        return LogGuard(None);
    }
    LogGuard(guard)
}

// ------------------------------------------------------------ start-up

/// Command-line options that shape start-up (`main.rs` fills these in).
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// `--socket <path>`: also moves the lock file beside it (§2).
    pub socket: Option<PathBuf>,
    /// `--tcp <addr>`: also listen on loopback TCP for the Windows client
    /// (the WSL side of the Windows/WSL split). Must be loopback; anything
    /// else is refused at start-up because TCP peers carry no uid credential.
    pub tcp: Option<std::net::SocketAddr>,
    /// `--config <path>`.
    pub config: Option<PathBuf>,
    /// `--state-dir <path>`.
    pub state_dir: Option<PathBuf>,
    /// `--foreground`: also log to stderr.
    pub foreground: bool,
    /// `--no-idle-exit`.
    pub no_idle_exit: bool,
    /// `-v` / `-vv`.
    pub verbosity: u8,
}

/// Resolves [`Paths`] from the process environment with the CLI overrides
/// applied.
///
/// `--socket` also relocates the runtime directory, because the lock file must
/// travel with the socket or two daemons on two sockets would fight over one
/// lock (§2).
#[must_use]
pub fn resolve_paths(options: &Options) -> Paths {
    let socket = options.socket.clone();
    let runtime = socket
        .as_ref()
        .and_then(|s| s.parent().map(Path::to_path_buf));
    let state = options.state_dir.clone();
    let config = options.config.clone();

    Paths::from_lookup(Platform::current(), st_config::current_uid(), move |key| {
        let override_for =
            |value: &Option<PathBuf>| value.as_ref().map(|p| p.clone().into_os_string());
        match key {
            "SUPERTERMINAL_SOCKET" if socket.is_some() => override_for(&socket),
            "SUPERTERMINAL_RUNTIME_DIR" if runtime.is_some() => override_for(&runtime),
            "SUPERTERMINAL_STATE_DIR" if state.is_some() => override_for(&state),
            "SUPERTERMINAL_CONFIG" if config.is_some() => override_for(&config),
            other => std::env::var_os(other),
        }
    })
}

/// Builds and starts a daemon.
///
/// Tests use this directly with a [`NullSpawner`] and a temporary [`Paths`];
/// [`run`] is the same thing wired to the real CLI, signals and logging.
pub struct ServerBuilder {
    paths: Paths,
    config: Config,
    spawner: Arc<dyn SurfaceSpawner>,
    data: Option<DataAcceptor>,
    build_id: String,
    check_peer_uid: bool,
    idle_exit: Option<Duration>,
    persist_debounce: Duration,
    tcp: Option<std::net::SocketAddr>,
}

impl ServerBuilder {
    /// A builder with defaults: the null spawner, no data plane, the peer-uid
    /// check on, and the idle timeout from `[server]`.
    #[must_use]
    pub fn new(paths: Paths, config: Config) -> Self {
        let idle_exit = idle_duration(config.server.idle_exit_minutes);
        Self {
            paths,
            config,
            spawner: Arc::new(NullSpawner::new()),
            data: None,
            build_id: crate::build_id(),
            check_peer_uid: true,
            idle_exit,
            persist_debounce: persist::DEBOUNCE,
            tcp: None,
        }
    }

    /// Replaces the Surface spawner (`st-core` in production).
    #[must_use]
    pub fn spawner(mut self, spawner: Arc<dyn SurfaceSpawner>) -> Self {
        self.spawner = spawner;
        self
    }

    /// Installs the data plane's accept hook.
    #[must_use]
    pub fn data_acceptor(mut self, acceptor: DataAcceptor) -> Self {
        self.data = Some(acceptor);
        self
    }

    /// Overrides the build id reported to clients.
    #[must_use]
    pub fn build_id(mut self, build_id: impl Into<String>) -> Self {
        self.build_id = build_id.into();
        self
    }

    /// Turns the `SO_PEERCRED` check off. Only for tests.
    #[must_use]
    pub fn check_peer_uid(mut self, check: bool) -> Self {
        self.check_peer_uid = check;
        self
    }

    /// Overrides the idle timeout; `None` disables idle exit (`--no-idle-exit`).
    #[must_use]
    pub fn idle_exit(mut self, idle: Option<Duration>) -> Self {
        self.idle_exit = idle;
        self
    }

    /// Also listens on loopback TCP (`--tcp`). Only for the Windows/WSL
    /// split: the Windows client has no Unix socket to dial.
    #[must_use]
    pub fn tcp(mut self, addr: Option<std::net::SocketAddr>) -> Self {
        self.tcp = addr;
        self
    }

    /// Overrides the persistence debounce. Only for tests.
    #[must_use]
    pub fn persist_debounce(mut self, debounce: Duration) -> Self {
        self.persist_debounce = debounce;
        self
    }

    /// Performs the whole start-up sequence.
    pub async fn start(self) -> anyhow::Result<RunningServer> {
        let lock_path = self.paths.lock_path();
        let lock = LockFile::acquire(&lock_path)?;
        tracing::debug!(path = %lock_path.display(), "took the single-instance lock");

        let socket_path = self.paths.ensure_socket_path()?;
        let listener = bind_socket(&socket_path)?;

        let workspace_file = self.paths.workspace_file()?;
        self.paths.ensure_state_dir()?;

        let metrics = Arc::new(Metrics::new());
        let uptime = Uptime::start();
        let shutdown = Shutdown::new();
        let persister = Persister::spawn_with_debounce(
            workspace_file.clone(),
            Arc::clone(&metrics),
            self.persist_debounce,
        );

        let defaults = SpawnDefaults::from_config(&self.config);
        let workspace = load_or_seed(
            &workspace_file,
            self.spawner.as_ref(),
            &defaults,
            metrics.as_ref(),
        );

        let on_shutdown = {
            let shutdown = shutdown.clone();
            Box::new(move |reason: String| shutdown.trigger(reason))
                as Box<dyn FnOnce(String) + Send>
        };

        let handle = WorkspaceActor::spawn(ActorConfig {
            workspace,
            spawner: Arc::clone(&self.spawner),
            defaults,
            persist: persister,
            metrics: Arc::clone(&metrics),
            uptime,
            build_id: self.build_id.clone(),
            on_shutdown: Some(on_shutdown),
        });

        let allowed_uid = self.check_peer_uid.then(st_config::current_uid);
        let mut ctx = ServerContext::new(
            handle.clone(),
            Arc::clone(&metrics),
            self.build_id,
            uptime,
            allowed_uid,
            shutdown.clone(),
        );
        ctx.data = self.data;
        let ctx = Arc::new(ctx);

        let accept = tokio::spawn(control::accept_loop(listener, Arc::clone(&ctx)));

        let tcp_listener = match self.tcp {
            Some(addr) if addr.ip().is_loopback() => {
                let listener = tokio::net::TcpListener::bind(addr)
                    .await
                    .map_err(|e| anyhow::anyhow!("cannot bind TCP {addr}: {e}"))?;
                Some(listener)
            }
            Some(addr) => {
                return Err(anyhow::anyhow!(
                    "refusing to bind TCP {addr}: only loopback addresses are allowed,                      TCP peers carry no uid credential"
                ));
            }
            None => None,
        };
        let tcp_listener_addr = tcp_listener
            .as_ref()
            .and_then(|listener| listener.local_addr().map(|a| a.to_string()).ok());
        let accept_tcp = tcp_listener
            .map(|listener| tokio::spawn(control::accept_loop_tcp(listener, Arc::clone(&ctx))));

        let idle = self
            .idle_exit
            .map(|period| tokio::spawn(idle_timer(Arc::clone(&ctx), period)));

        let tcp_bound = accept_tcp
            .as_ref()
            .and(tcp_listener_addr.as_deref())
            .unwrap_or("");
        tracing::info!(
            socket = %socket_path.display(),
            tcp = tcp_bound,
            workspace = %workspace_file.display(),
            pid = std::process::id(),
            "superterminald ready"
        );

        Ok(RunningServer {
            ctx,
            workspace: handle,
            socket_path,
            workspace_file,
            lock,
            accept,
            accept_tcp,
            idle,
        })
    }
}

/// Converts `server.idle_exit_minutes` into a duration; `0` disables it.
#[must_use]
pub fn idle_duration(minutes: f64) -> Option<Duration> {
    if minutes.is_finite() && minutes > 0.0 {
        Some(Duration::from_secs_f64(minutes * 60.0))
    } else {
        None
    }
}

fn bind_socket(path: &Path) -> anyhow::Result<UnixListener> {
    // Safe because we already hold the flock: no live daemon can be serving
    // this socket (§2, step 4).
    match std::fs::remove_file(path) {
        Ok(()) => tracing::info!(path = %path.display(), "removed a stale socket"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(anyhow::anyhow!(
                "cannot remove the stale socket {}: {e}",
                path.display()
            ))
        }
    }

    let listener = UnixListener::bind(path)
        .map_err(|e| anyhow::anyhow!("cannot bind {}: {e}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(SOCKET_MODE))
            .map_err(|e| anyhow::anyhow!("cannot chmod {}: {e}", path.display()))?;
    }

    Ok(listener)
}

/// Loads `workspace.json` and re-seeds it, or starts with one `Default`
/// Session holding one Tab (§2 steps 5–6, grilling Q21/Q48).
fn load_or_seed(
    path: &Path,
    spawner: &dyn SurfaceSpawner,
    defaults: &SpawnDefaults,
    metrics: &Metrics,
) -> Workspace {
    let mut ws = Workspace::new();

    match persist::load(path) {
        persist::Loaded::File(file) => {
            crate::workspace::reseed(&file, &mut ws, spawner, defaults, metrics);
            tracing::info!(
                path = %path.display(),
                sessions = ws.sessions.len(),
                "re-seeded the saved workspace"
            );
        }
        persist::Loaded::Missing => {
            tracing::info!(path = %path.display(), "no saved workspace; starting fresh");
        }
        persist::Loaded::Corrupt { moved_to, reason } => {
            tracing::warn!(
                path = %path.display(),
                moved_to = %moved_to.display(),
                reason,
                "the saved workspace could not be read; starting fresh"
            );
        }
    }

    if ws.sessions.is_empty() {
        match spawner.spawn(&defaults.seed_spec()) {
            Ok(spawned) => {
                metrics.surfaces_spawned.inc();
                ws.insert_surface(crate::workspace::Surface {
                    id: spawned.id,
                    title: spawned
                        .title
                        .unwrap_or_else(|| defaults.seed_spec().program_name()),
                    user_title: None,
                    cwd: Some(defaults.cwd.display().to_string()),
                    shell: defaults.shell.clone(),
                    cols: defaults.cols,
                    rows: defaults.rows,
                    has_foreground_child: false,
                    status: crate::workspace::SurfaceStatus::Running { pid: spawned.pid },
                    view: st_proto::ViewState::default(),
                    pristine: true,
                });
                ws.seed_default_session(spawned.id);
            }
            Err(e) => tracing::error!(error = %e, "cannot spawn the initial shell"),
        }
    }

    ws
}

/// The idle-exit timer (§2, grilling Q42).
///
/// The daemon exits when it has had no connections *and* no non-pristine
/// Surfaces for `period`. Pristine Surfaces — the auto-seeded shells that
/// Q21 keeps re-creating — count as zero, otherwise the daemon would never be
/// idle.
async fn idle_timer(ctx: Arc<ServerContext>, period: Duration) {
    let tick = (period / 10).clamp(Duration::from_millis(25), Duration::from_secs(30));
    let mut stop = ctx.shutdown.subscribe();

    loop {
        tokio::select! {
            () = tokio::time::sleep(tick) => {}
            _ = stop.wait() => return,
        }

        if ctx.open_connections() > 0 {
            ctx.mark_active();
            continue;
        }
        let Ok(stats) = ctx.workspace.stats().await else {
            return;
        };
        if stats.busy_surfaces > 0 {
            ctx.mark_active();
            continue;
        }
        if ctx.idle_for() >= period {
            ctx.shutdown.trigger(format!(
                "idle for {:.0}s with no connections and no live work",
                period.as_secs_f64()
            ));
            return;
        }
    }
}

// ------------------------------------------------------------ running server

/// A started daemon. Dropping it releases the lock but does *not* shut down
/// cleanly — call [`RunningServer::wait`] or [`RunningServer::stop`].
pub struct RunningServer {
    ctx: Arc<ServerContext>,
    workspace: WorkspaceHandle,
    socket_path: PathBuf,
    workspace_file: PathBuf,
    lock: LockFile,
    accept: JoinHandle<()>,
    accept_tcp: Option<JoinHandle<()>>,
    idle: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for RunningServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningServer")
            .field("socket_path", &self.socket_path)
            .field("workspace_file", &self.workspace_file)
            .field("lock_path", &self.lock.path())
            .finish_non_exhaustive()
    }
}

impl RunningServer {
    /// The shared connection context.
    #[must_use]
    pub fn context(&self) -> &Arc<ServerContext> {
        &self.ctx
    }

    /// The Workspace actor handle (the data plane's entry point).
    #[must_use]
    pub fn workspace(&self) -> &WorkspaceHandle {
        &self.workspace
    }

    /// The socket clients dial.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// The document being persisted.
    #[must_use]
    pub fn workspace_file(&self) -> &Path {
        &self.workspace_file
    }

    /// The lock file held for the life of this daemon.
    #[must_use]
    pub fn lock_path(&self) -> &Path {
        self.lock.path()
    }

    /// The shutdown trigger, so callers can stop the daemon from elsewhere.
    #[must_use]
    pub fn shutdown_trigger(&self) -> Shutdown {
        self.ctx.shutdown.clone()
    }

    /// Waits for a shutdown trigger and then performs the graceful sequence.
    /// Returns the reason it stopped.
    pub async fn wait(self) -> anyhow::Result<String> {
        let mut watch = self.ctx.shutdown.subscribe();
        let reason = watch.wait().await;
        self.finish(reason.clone()).await?;
        Ok(reason)
    }

    /// Triggers a shutdown and performs it.
    pub async fn stop(self, reason: impl Into<String>) -> anyhow::Result<()> {
        let reason = reason.into();
        self.ctx.shutdown.trigger(reason.clone());
        self.finish(reason).await
    }

    async fn finish(self, reason: String) -> anyhow::Result<()> {
        // 1. stop accepting.
        self.accept.abort();
        if let Some(accept_tcp) = self.accept_tcp {
            accept_tcp.abort();
        }
        if let Some(idle) = self.idle {
            idle.abort();
        }

        // 2. tell everyone, flush workspace.json, signal the Surfaces.
        if self.workspace.shutdown(reason).await.is_err() {
            tracing::warn!("the workspace actor was already gone at shutdown");
        }
        tokio::time::sleep(SHUTDOWN_GRACE).await;

        // 3. remove the socket, then release the lock by dropping the file.
        if let Err(e) = std::fs::remove_file(&self.socket_path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %self.socket_path.display(), error = %e, "cannot unlink the socket");
            }
        }
        drop(self.lock);
        tracing::info!("superterminald stopped");
        Ok(())
    }
}

// ------------------------------------------------------------ signals

/// Installs the signal handlers described in §2.
///
/// * `SIGTERM`/`SIGINT` — graceful shutdown.
/// * `SIGHUP` — reload `config.toml`. The daemon deliberately does **not** die
///   on a hangup: it must outlive the terminal that spawned it (grilling Q30).
///   Only `[server].idle_exit_minutes` can change at runtime today; a changed
///   `[shell]` applies to the next Surface spawned.
pub fn install_signal_handlers(shutdown: Shutdown, paths: Paths) -> anyhow::Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    let mut hup = signal(SignalKind::hangup())?;

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = term.recv() => {
                    shutdown.trigger("SIGTERM");
                    return;
                }
                _ = int.recv() => {
                    shutdown.trigger("SIGINT");
                    return;
                }
                _ = hup.recv() => reload_config(&paths),
            }
        }
    });

    Ok(())
}

fn reload_config(paths: &Paths) {
    match paths.config_path().map(Config::load_from_verbose) {
        Ok(Ok(loaded)) => {
            for warning in &loaded.warnings {
                tracing::warn!(warning, "config");
            }
            tracing::info!(
                path = %loaded.path.display(),
                idle_exit_minutes = loaded.config.server.idle_exit_minutes,
                "reloaded config.toml on SIGHUP"
            );
        }
        Ok(Err(e)) => tracing::warn!(error = %e, "cannot reload config.toml"),
        Err(e) => tracing::warn!(error = %e, "cannot locate config.toml"),
    }
}

// ------------------------------------------------------------ run

/// The daemon's whole life, as `main` runs it.
pub async fn run(options: Options) -> anyhow::Result<()> {
    let paths = resolve_paths(&options);
    let _log_guard = init_logging(&paths, options.foreground, options.verbosity);

    let config_path = paths.config_path()?;
    let loaded = Config::load_from_verbose(&config_path)?;
    for warning in &loaded.warnings {
        tracing::warn!(warning, path = %config_path.display(), "config");
    }

    // The data plane: one supervisor is both the `SurfaceSpawner` the actor
    // drives and the owner of every live Surface the DATA connections serve
    // (`src/supervisor.rs`, `src/data/mod.rs`). Its notifier is deferred
    // because the actor it reports to does not exist until `start()` runs.
    let notifier = DeferredNotifier::new();
    let supervisor = Arc::new(SurfaceSupervisor::new(
        SupervisorConfig::from_config(&loaded.config, crate::build_id(), Some(paths.socket_path())),
        Arc::clone(&notifier) as Arc<dyn WorkspaceNotifier>,
    ));

    let mut builder = ServerBuilder::new(paths.clone(), loaded.config)
        .spawner(Arc::clone(&supervisor) as Arc<dyn SurfaceSpawner>)
        .data_acceptor(crate::data::acceptor(Arc::clone(&supervisor)))
        .tcp(options.tcp);
    if options.no_idle_exit {
        builder = builder.idle_exit(None);
    }

    let server = match builder.start().await {
        Ok(server) => server,
        Err(e) => {
            // Losing the lock race is the normal outcome of two clients
            // starting at once (grilling Q30): say so and exit 0.
            if let Some(LockError::AlreadyRunning { pid, .. }) = e.downcast_ref::<LockError>() {
                tracing::info!(?pid, "another superterminald is already running; exiting");
                eprintln!("superterminald: {e}");
                return Ok(());
            }
            return Err(e);
        }
    };

    supervisor.install_metrics(Arc::clone(&server.context().metrics));
    notifier.attach(server.workspace().clone());
    // Start the 120 Hz pump now rather than on the first DATA connection: it
    // also reaps exited children and reports title/cwd changes upward, which
    // the Workspace needs whether or not anyone is watching a Surface.
    supervisor.ensure_pump();

    install_signal_handlers(server.shutdown_trigger(), paths)?;
    let reason = server.wait().await?;
    supervisor.shutdown().await;
    tracing::info!(reason, "exit");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_is_exclusive_and_carries_the_pid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");

        let first = LockFile::acquire(&path).unwrap();
        assert_eq!(read_pid(&path), Some(std::process::id()));

        match LockFile::acquire(&path) {
            Err(LockError::AlreadyRunning { pid, .. }) => {
                assert_eq!(pid, Some(std::process::id()));
            }
            other => panic!("expected AlreadyRunning, got {other:?}"),
        }

        drop(first);
        assert!(
            LockFile::acquire(&path).is_ok(),
            "the lock is released on drop"
        );
    }

    #[test]
    fn idle_minutes_convert_and_zero_disables() {
        assert_eq!(idle_duration(0.0), None);
        assert_eq!(idle_duration(-1.0), None);
        assert_eq!(idle_duration(0.05), Some(Duration::from_secs(3)));
        assert_eq!(idle_duration(15.0), Some(Duration::from_secs(900)));
    }

    #[test]
    fn cli_overrides_move_the_socket_and_the_lock_together() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("test.sock");
        let paths = resolve_paths(&Options {
            socket: Some(socket.clone()),
            state_dir: Some(dir.path().join("state")),
            ..Options::default()
        });
        assert_eq!(paths.socket_path(), socket);
        assert_eq!(paths.lock_path(), dir.path().join("lock"));
        assert_eq!(
            paths.workspace_file().unwrap(),
            dir.path().join("state").join("workspace.json")
        );
    }

    #[tokio::test]
    async fn the_shutdown_trigger_keeps_the_first_reason() {
        let shutdown = Shutdown::new();
        let mut watch = shutdown.subscribe();
        assert!(!shutdown.is_triggered());
        shutdown.trigger("first");
        shutdown.trigger("second");
        assert!(shutdown.is_triggered());
        assert_eq!(watch.wait().await, "first");
        assert_eq!(shutdown.reason().as_deref(), Some("first"));
    }

    #[test]
    fn binding_replaces_a_stale_socket_and_chmods_it() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.sock");
        std::fs::write(&path, b"stale").unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _listener = runtime.block_on(async { bind_socket(&path).unwrap() });

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, SOCKET_MODE);
    }
}
