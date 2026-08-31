//! The single-writer Workspace actor — `docs/plan/03-server.md` §3.
//!
//! Exactly one task owns the [`Workspace`]. Every mutation arrives as a
//! [`WorkspaceCommand`] on a bounded mpsc channel, so mutations are *totally
//! ordered*; each successful one bumps the revision and publishes one
//! [`Ev::Workspace`] on a `tokio::sync::broadcast`, which is how every
//! connected client converges (§3, "why not `Arc<Mutex<Workspace>>`").
//!
//! Both planes funnel into the same commands:
//!
//! * the control plane sends [`WorkspaceCommand::Request`] with the parsed
//!   [`Req`], including `view.set`;
//! * the data plane (grilling Q43/Q49) calls
//!   [`WorkspaceHandle::set_view_state`], which builds the very same
//!   [`Req::ViewSet`] and sends the very same command — there is one code path
//!   for View State, not two.
//!
//! The actor never touches a PTY: processes are started and signalled through
//! the [`SurfaceSpawner`] seam (see [`crate::workspace::spawn`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use st_proto::control::{
    Empty, ErrorBody, ErrorCode, Ev, KillSignal, Revision, Selection, SessionCreated, SessionList,
    SurfaceCreated, TabCreated,
};
use st_proto::{Req, SessionId, SurfaceId, TabId};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::metrics::{Metrics, Uptime};
use crate::persist::{self, Persister};
use crate::workspace::model::{Surface, SurfaceStatus, Workspace};
use crate::workspace::spawn::{SpawnSpec, SurfaceSpawner};

/// Identifies one connection, so the actor can suppress the echo of a
/// view-only change back to the connection that made it (`02-protocol.md`
/// §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientId(pub u64);

/// Capacity of the command channel. Deep enough that a burst of control
/// requests never blocks an accept, shallow enough to apply backpressure.
pub const COMMAND_CAPACITY: usize = 256;

/// Capacity of the broadcast channel. A connection that falls this far behind
/// gets a `Lagged` and re-reads the whole document, which is correct because
/// `ev.workspace` is a full snapshot.
pub const EVENT_CAPACITY: usize = 64;

/// One published event, plus the routing metadata connections need.
#[derive(Debug, Clone)]
pub struct EventEnvelope {
    /// The typed event.
    pub ev: Ev,
    /// The event as it goes on the wire, with the `pid` extension applied to
    /// Surface metadata (see [`snapshot_value`]).
    pub json: Value,
    /// The Workspace revision this event reflects, when it has one. A
    /// connection that subscribed at revision *r* drops events at or below it,
    /// because its `workspace.subscribe` result already contained them.
    pub revision: Option<Revision>,
    /// The connection that caused the change, when it must not be echoed back
    /// (`view.set`: "the server does not echo `ev.workspace` for view-only
    /// changes to the originating connection").
    pub suppress: Option<ClientId>,
}

/// Everything the actor can be asked to do.
///
/// The data-plane agent adds no variants of its own: Surface reports come in
/// through [`WorkspaceCommand::SurfaceEvent`], and View State edits through
/// [`WorkspaceCommand::Request`] with a [`Req::ViewSet`].
#[derive(Debug)]
pub enum WorkspaceCommand {
    /// A control-plane request, or a data-plane `SetViewState` rendered as one.
    Request {
        /// The request.
        req: Box<Req>,
        /// The connection it came from, when it should not see its own echo.
        origin: Option<ClientId>,
        /// Where the answer goes.
        reply: oneshot::Sender<Result<Value, ErrorBody>>,
    },
    /// A report from the Surface supervisor about one Surface.
    SurfaceEvent(SurfaceEvent),
    /// Read-only: what the idle timer and `server.status` need.
    Stats {
        /// Where the answer goes.
        reply: oneshot::Sender<Stats>,
    },
    /// Persist immediately and acknowledge, bypassing the debounce (§8).
    Flush {
        /// Signalled once the document is on disk.
        reply: oneshot::Sender<()>,
    },
    /// Tell every client the daemon is going away, then stop.
    Shutdown {
        /// Text for `ev.server_shutting_down`.
        reason: String,
        /// Signalled once the notice has been published and the document
        /// flushed.
        reply: oneshot::Sender<()>,
    },
}

/// What the Surface supervisor reports upward (`03-server.md` §3, last
/// paragraph). Owned by the data-plane agent; the control plane only reacts.
#[derive(Debug, Clone)]
pub enum SurfaceEvent {
    /// The program set a new title (OSC 0/2).
    Title {
        /// Which Surface.
        surface: SurfaceId,
        /// The new title.
        title: String,
    },
    /// The working directory changed (OSC 7 or the `/proc` probe).
    Cwd {
        /// Which Surface.
        surface: SurfaceId,
        /// The new directory.
        cwd: String,
    },
    /// The grid was resized (last resize wins, grilling Q40).
    Resized {
        /// Which Surface.
        surface: SurfaceId,
        /// New width.
        cols: u16,
        /// New height.
        rows: u16,
    },
    /// Whether a program other than the shell is in the foreground (Q48).
    /// A foreground child also ends pristineness (Q42).
    ForegroundChild {
        /// Which Surface.
        surface: SurfaceId,
        /// Whether one is present.
        present: bool,
    },
    /// The Surface received input, so it is no longer pristine (Q42).
    Input {
        /// Which Surface.
        surface: SurfaceId,
    },
    /// The Surface's process ended (grilling Q22: nothing auto-closes).
    Exited {
        /// Which Surface.
        surface: SurfaceId,
        /// Exit code, when it exited normally.
        code: Option<i32>,
        /// Signal name, when it was killed.
        signal: Option<String>,
    },
}

/// The counts the idle timer (grilling Q42) and `server.status` need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Current Workspace revision.
    pub revision: Revision,
    /// Surfaces whose process is alive.
    pub live_surfaces: usize,
    /// Live Surfaces that are not pristine — the ones that keep the daemon up.
    pub busy_surfaces: usize,
}

/// Defaults applied to a [`st_proto::SpawnSpec`] that leaves fields out.
#[derive(Debug, Clone)]
pub struct SpawnDefaults {
    /// argv from `[shell]` in `config.toml`, already resolved by `st-config`.
    pub shell: Vec<String>,
    /// Fallback working directory (`$HOME`, else `/`).
    pub cwd: PathBuf,
    /// Grid width used when a client asks for zero.
    pub cols: u16,
    /// Grid height used when a client asks for zero.
    pub rows: u16,
}

impl Default for SpawnDefaults {
    fn default() -> Self {
        Self {
            shell: vec!["/bin/sh".to_string()],
            cwd: std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from),
            cols: 80,
            rows: 24,
        }
    }
}

impl SpawnDefaults {
    /// Reads `[shell]` out of a loaded configuration.
    #[must_use]
    pub fn from_config(config: &st_config::Config) -> Self {
        let resolved = config.resolve_shell();
        let mut argv = vec![resolved.program.to_string_lossy().into_owned()];
        argv.extend(resolved.args.iter().cloned());
        Self {
            shell: argv,
            ..Self::default()
        }
    }
}

/// A cheap, cloneable handle onto the actor.
#[derive(Debug, Clone)]
pub struct WorkspaceHandle {
    cmds: mpsc::Sender<WorkspaceCommand>,
    events: broadcast::Sender<EventEnvelope>,
}

/// The error returned when the actor task is gone (only during shutdown).
#[derive(Debug, thiserror::Error)]
#[error("the workspace actor has stopped")]
pub struct ActorGone;

impl WorkspaceHandle {
    /// Subscribes to [`EventEnvelope`]s. Every connection holds one.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
    }

    /// Applies one request and returns its `result` value, or the error body
    /// to put in the `err` envelope.
    pub async fn request(&self, req: Req, origin: Option<ClientId>) -> Result<Value, ErrorBody> {
        let (reply, answer) = oneshot::channel();
        self.cmds
            .send(WorkspaceCommand::Request {
                req: Box::new(req),
                origin,
                reply,
            })
            .await
            .map_err(|_| shutting_down())?;
        answer.await.map_err(|_| shutting_down())?
    }

    /// The View State entry point shared by both planes (grilling Q43/Q49).
    ///
    /// The data plane's `SetViewState` message must land here so the edit goes
    /// through the same command, the same revision bump and the same
    /// `ev.workspace` echo as the control plane's `view.set`.
    pub async fn set_view_state(
        &self,
        surface: SurfaceId,
        scroll_offset: Option<u32>,
        selection: Option<Option<Selection>>,
        origin: Option<ClientId>,
    ) -> Result<Value, ErrorBody> {
        self.request(
            Req::ViewSet {
                id: 0,
                surface,
                scroll_offset,
                selection,
            },
            origin,
        )
        .await
    }

    /// Reports something about a Surface (data plane).
    pub async fn surface_event(&self, event: SurfaceEvent) -> Result<(), ActorGone> {
        self.cmds
            .send(WorkspaceCommand::SurfaceEvent(event))
            .await
            .map_err(|_| ActorGone)
    }

    /// The current document, as `workspace.get` would return it.
    pub async fn snapshot(&self) -> Result<Value, ActorGone> {
        self.request(Req::WorkspaceGet { id: 0 }, None)
            .await
            .map_err(|_| ActorGone)
    }

    /// Counts for the idle timer and `server.status`.
    pub async fn stats(&self) -> Result<Stats, ActorGone> {
        let (reply, answer) = oneshot::channel();
        self.cmds
            .send(WorkspaceCommand::Stats { reply })
            .await
            .map_err(|_| ActorGone)?;
        answer.await.map_err(|_| ActorGone)
    }

    /// Writes `workspace.json` now and waits for it.
    pub async fn flush(&self) -> Result<(), ActorGone> {
        let (reply, answer) = oneshot::channel();
        self.cmds
            .send(WorkspaceCommand::Flush { reply })
            .await
            .map_err(|_| ActorGone)?;
        answer.await.map_err(|_| ActorGone)
    }

    /// Publishes `ev.server_shutting_down`, flushes, and stops the actor.
    pub async fn shutdown(&self, reason: impl Into<String>) -> Result<(), ActorGone> {
        let (reply, answer) = oneshot::channel();
        self.cmds
            .send(WorkspaceCommand::Shutdown {
                reason: reason.into(),
                reply,
            })
            .await
            .map_err(|_| ActorGone)?;
        answer.await.map_err(|_| ActorGone)
    }
}

fn shutting_down() -> ErrorBody {
    ErrorBody::new(ErrorCode::ShuttingDown, "the server is shutting down")
}

/// The actor task's state.
pub struct WorkspaceActor {
    ws: Workspace,
    spawner: Arc<dyn SurfaceSpawner>,
    defaults: SpawnDefaults,
    persist: Persister,
    metrics: Arc<Metrics>,
    uptime: Uptime,
    build_id: String,
    events: broadcast::Sender<EventEnvelope>,
    cmds: mpsc::Receiver<WorkspaceCommand>,
    shutdown: Option<Box<dyn FnOnce(String) + Send>>,
}

/// Everything [`WorkspaceActor::spawn`] needs.
pub struct ActorConfig {
    /// The Workspace to start from, already re-seeded (see
    /// [`crate::workspace::reseed`]).
    pub workspace: Workspace,
    /// How Surfaces are started and signalled.
    pub spawner: Arc<dyn SurfaceSpawner>,
    /// Defaults for a partial `SpawnSpec`.
    pub defaults: SpawnDefaults,
    /// Where `workspace.json` is written.
    pub persist: Persister,
    /// Shared counters.
    pub metrics: Arc<Metrics>,
    /// Daemon start time, for `server.status`.
    pub uptime: Uptime,
    /// Build id echoed in `HelloAck` and `server.status`.
    pub build_id: String,
    /// Called once when a client asks the daemon to stop; the lifecycle layer
    /// turns it into the same graceful path as `SIGTERM`.
    pub on_shutdown: Option<Box<dyn FnOnce(String) + Send>>,
}

impl WorkspaceActor {
    /// Starts the actor task and returns its handle.
    pub fn spawn(config: ActorConfig) -> WorkspaceHandle {
        let (tx, rx) = mpsc::channel(COMMAND_CAPACITY);
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let actor = Self {
            ws: config.workspace,
            spawner: config.spawner,
            defaults: config.defaults,
            persist: config.persist,
            metrics: config.metrics,
            uptime: config.uptime,
            build_id: config.build_id,
            events: events.clone(),
            cmds: rx,
            shutdown: config.on_shutdown,
        };
        tokio::spawn(actor.run());
        WorkspaceHandle { cmds: tx, events }
    }

    async fn run(mut self) {
        while let Some(cmd) = self.cmds.recv().await {
            match cmd {
                WorkspaceCommand::Request { req, origin, reply } => {
                    let answer = self.handle_request(&req, origin);
                    let _ = reply.send(answer);
                }
                WorkspaceCommand::SurfaceEvent(event) => self.handle_surface_event(event),
                WorkspaceCommand::Stats { reply } => {
                    let _ = reply.send(self.stats());
                }
                WorkspaceCommand::Flush { reply } => {
                    self.persist.save(persist::snapshot_file(&self.ws));
                    self.persist.flush().await;
                    let _ = reply.send(());
                }
                WorkspaceCommand::Shutdown { reason, reply } => {
                    self.publish(
                        Ev::ServerShuttingDown {
                            reason: reason.clone(),
                        },
                        None,
                        None,
                    );
                    self.persist.save(persist::snapshot_file(&self.ws));
                    self.persist.flush().await;
                    // §2: `killpg(pgid, SIGHUP)` every Surface. The spawner
                    // owns the grace period and the follow-up SIGKILL.
                    let live: Vec<SurfaceId> = self
                        .ws
                        .surfaces
                        .values()
                        .filter(|s| s.status.is_running())
                        .map(|s| s.id)
                        .collect();
                    for surface in live {
                        self.kill_surface(surface, KillSignal::Hup);
                    }
                    let _ = reply.send(());
                    tracing::info!(reason, "workspace actor stopped");
                    return;
                }
            }
        }
    }

    fn stats(&self) -> Stats {
        Stats {
            revision: self.ws.revision(),
            live_surfaces: self.ws.live_surfaces(),
            busy_surfaces: self.ws.busy_surfaces(),
        }
    }

    // ------------------------------------------------------------ requests

    fn handle_request(&mut self, req: &Req, origin: Option<ClientId>) -> Result<Value, ErrorBody> {
        let before = self.ws.revision();
        let view_only = matches!(req, Req::ViewSet { .. });
        let result = self.apply(req, origin);

        if self.ws.revision() != before {
            self.metrics.revisions.inc();
            self.persist.save(persist::snapshot_file(&self.ws));
            let (ev, json) = workspace_event(&self.ws);
            let suppress = if view_only { origin } else { None };
            self.publish_prepared(ev, json, Some(self.ws.revision()), suppress);
        }
        result
    }

    #[allow(clippy::too_many_lines)]
    fn apply(&mut self, req: &Req, _origin: Option<ClientId>) -> Result<Value, ErrorBody> {
        self.ws.check_revision(req.if_revision())?;

        match req {
            Req::WorkspaceGet { .. } | Req::WorkspaceSubscribe { .. } => {
                Ok(snapshot_value(&self.ws))
            }

            Req::SessionCreate { name, .. } => {
                let session = self.ws.create_session(name.clone());
                let revision = self.ws.bump_revision();
                value(SessionCreated { session, revision })
            }

            Req::SessionRename { session, name, .. } => {
                self.ws.rename_session(*session, name.clone())?;
                self.revision_result()
            }

            Req::SessionDelete { session, .. } => {
                let (surfaces, needs_reseed) = self.ws.delete_session(*session)?;
                for surface in surfaces {
                    self.kill_surface(surface, KillSignal::Hup);
                }
                if needs_reseed {
                    self.reseed_default()?;
                }
                self.revision_result()
            }

            Req::SessionList { .. } => value(SessionList {
                sessions: self.ws.document().sessions,
            }),

            Req::SessionSetActive { session, .. } => {
                self.ws.set_active_session(*session)?;
                self.revision_result()
            }

            Req::TabCreate {
                session,
                index,
                spawn,
                surface,
                ..
            } => {
                self.ws.session(*session)?;
                let surface = match (spawn, surface) {
                    (Some(spec), None) => self.spawn_surface(spec, false)?,
                    (None, Some(existing)) => {
                        self.ws.surface(*existing)?;
                        if self.ws.surface_is_attached(*existing) {
                            return Err(ErrorBody::new(
                                ErrorCode::BadRequest,
                                format!("surface {existing} is already shown in a tab"),
                            ));
                        }
                        *existing
                    }
                    _ => {
                        return Err(ErrorBody::new(
                            ErrorCode::BadRequest,
                            "tab.create needs exactly one of `spawn` and `surface`",
                        ))
                    }
                };
                let tab = self.ws.alloc_tab_id();
                self.ws.insert_tab(*session, *index, tab, surface)?;
                let revision = self.ws.bump_revision();
                value(TabCreated {
                    tab,
                    surface,
                    revision,
                })
            }

            Req::TabClose { tab, .. } => {
                let closed = self.ws.close_tab(*tab)?;
                self.kill_surface(closed.surface, KillSignal::Hup);
                if closed.needs_reseed {
                    self.reseed_default()?;
                }
                self.revision_result()
            }

            Req::TabReorder { tab, index, .. } => {
                self.ws.reorder_tab(*tab, *index)?;
                self.revision_result()
            }

            Req::TabMove {
                tab,
                to_session,
                index,
                ..
            } => {
                self.ws.move_tab(*tab, *to_session, *index)?;
                self.revision_result()
            }

            Req::TabSetActive { tab, .. } => {
                self.ws.set_active_tab(*tab)?;
                self.revision_result()
            }

            Req::SurfaceCreate { spawn, .. } => {
                let surface = self.spawn_surface(spawn, false)?;
                self.ws.bump_revision();
                value(SurfaceCreated { surface })
            }

            Req::SurfaceKill {
                surface, signal, ..
            } => {
                self.ws.surface(*surface)?;
                self.spawner
                    .kill(*surface, signal.unwrap_or_default())
                    .map_err(|e| e.to_error_body())?;
                value(Empty {})
            }

            Req::SurfaceRename {
                surface,
                user_title,
                ..
            } => {
                self.ws.rename_surface(*surface, user_title.clone())?;
                self.revision_result()
            }

            Req::ViewSet {
                surface,
                scroll_offset,
                selection,
                ..
            } => {
                self.ws
                    .set_view_state(*surface, *scroll_offset, *selection)?;
                self.revision_result()
            }

            Req::ServerStatus { .. } => Ok(self.status_value()),

            Req::ServerShutdown { force, .. } => {
                let live = self.ws.live_surfaces();
                if live > 0 && !force.unwrap_or(false) {
                    return Err(ErrorBody {
                        code: ErrorCode::Conflict,
                        message: format!(
                            "{live} surface(s) are still running; retry with force: true"
                        ),
                        data: Some(json!({ "surfaces": live })),
                    });
                }
                if let Some(trigger) = self.shutdown.take() {
                    trigger("server.shutdown".to_string());
                }
                value(Empty {})
            }
        }
    }

    fn revision_result(&mut self) -> Result<Value, ErrorBody> {
        let revision = self.ws.bump_revision();
        Ok(json!({ "revision": revision }))
    }

    fn status_value(&self) -> Value {
        let status = st_proto::control::ServerStatus {
            build_id: self.build_id.clone(),
            proto_version: st_proto::PROTO_VERSION.to_string(),
            pid: std::process::id(),
            uptime_s: self.uptime.secs(),
            surfaces: u32::try_from(self.ws.live_surfaces()).unwrap_or(u32::MAX),
            control_clients: u32::try_from(self.metrics.control_clients.get()).unwrap_or(u32::MAX),
            data_clients: u32::try_from(self.metrics.data_clients.get()).unwrap_or(u32::MAX),
            workspace_file: self.persist.path().display().to_string(),
        };
        let mut value = serde_json::to_value(status).unwrap_or_else(|_| json!({}));
        if let Some(object) = value.as_object_mut() {
            object.insert("metrics".to_string(), self.metrics.to_json());
        }
        value
    }

    // ------------------------------------------------------------ surfaces

    fn spawn_surface(
        &mut self,
        spec: &st_proto::SpawnSpec,
        seeded: bool,
    ) -> Result<SurfaceId, ErrorBody> {
        let resolved = resolve_spawn(spec, &self.defaults, seeded)?;
        self.spawn_resolved(&resolved)
    }

    fn spawn_resolved(&mut self, spec: &SpawnSpec) -> Result<SurfaceId, ErrorBody> {
        let spawned = self.spawner.spawn(spec).map_err(|e| e.to_error_body())?;
        self.metrics.surfaces_spawned.inc();
        self.ws.insert_surface(Surface {
            id: spawned.id,
            title: spawned.title.unwrap_or_else(|| spec.program_name()),
            user_title: None,
            cwd: Some(spec.cwd.display().to_string()),
            shell: spec.shell.clone(),
            cols: spec.cols,
            rows: spec.rows,
            has_foreground_child: false,
            status: SurfaceStatus::Running { pid: spawned.pid },
            view: st_proto::ViewState::default(),
            pristine: spec.seeded,
        });
        Ok(spawned.id)
    }

    fn kill_surface(&self, surface: SurfaceId, signal: KillSignal) {
        if let Err(e) = self.spawner.kill(surface, signal) {
            tracing::warn!(%surface, error = %e, "cannot signal surface");
        }
        // Q21: closing a Tab destroys its Surface, so drop it from the
        // supervisor too. Signalling alone would leak the engine, the PTY
        // handles and any data-plane subscriptions for the daemon's lifetime.
        self.spawner.destroy(surface);
    }

    /// Grilling Q21: the Workspace always has at least one Session with one Tab.
    fn reseed_default(&mut self) -> Result<(SessionId, TabId), ErrorBody> {
        let spec = self.defaults.seed_spec();
        let surface = self.spawn_resolved(&spec)?;
        Ok(self.ws.seed_default_session(surface))
    }

    fn handle_surface_event(&mut self, event: SurfaceEvent) {
        let before = self.ws.revision();
        let mut exited = None;

        match event {
            SurfaceEvent::Title { surface, title } => {
                if let Ok(s) = self.ws.surface_mut(surface) {
                    if s.title != title {
                        s.title = title;
                        self.ws.bump_revision();
                    }
                }
            }
            SurfaceEvent::Cwd { surface, cwd } => {
                if let Ok(s) = self.ws.surface_mut(surface) {
                    if s.cwd.as_deref() != Some(cwd.as_str()) {
                        s.cwd = Some(cwd);
                        self.ws.bump_revision();
                    }
                }
            }
            SurfaceEvent::Resized {
                surface,
                cols,
                rows,
            } => {
                if let Ok(s) = self.ws.surface_mut(surface) {
                    if (s.cols, s.rows) != (cols, rows) {
                        s.cols = cols;
                        s.rows = rows;
                        // Grilling Q40: a resize clears the selection and the
                        // cleared View State is broadcast.
                        s.view.selection = None;
                        self.ws.bump_revision();
                    }
                }
            }
            SurfaceEvent::ForegroundChild { surface, present } => {
                if let Ok(s) = self.ws.surface_mut(surface) {
                    if s.has_foreground_child != present {
                        s.has_foreground_child = present;
                        if present {
                            s.pristine = false;
                        }
                        self.ws.bump_revision();
                    }
                }
            }
            SurfaceEvent::Input { surface } => {
                // Q42 only: pristineness is not part of the document, so this
                // deliberately does not bump the revision.
                self.ws.mark_dirty(surface);
            }
            SurfaceEvent::Exited {
                surface,
                code,
                signal,
            } => {
                if self.ws.mark_exited(surface, code, signal.clone()) {
                    self.metrics.surfaces_exited.inc();
                    self.ws.bump_revision();
                    exited = Some(Ev::SurfaceExited {
                        surface,
                        code,
                        signal,
                    });
                }
            }
        }

        if self.ws.revision() != before {
            self.metrics.revisions.inc();
            self.persist.save(persist::snapshot_file(&self.ws));
            if let Some(ev) = exited {
                self.publish(ev, None, None);
            }
            let (ev, json) = workspace_event(&self.ws);
            self.publish_prepared(ev, json, Some(self.ws.revision()), None);
        }
    }

    // ------------------------------------------------------------ events

    fn publish(&self, ev: Ev, revision: Option<Revision>, suppress: Option<ClientId>) {
        let json = serde_json::to_value(&ev).unwrap_or_else(|_| json!({}));
        self.publish_prepared(ev, json, revision, suppress);
    }

    fn publish_prepared(
        &self,
        ev: Ev,
        json: Value,
        revision: Option<Revision>,
        suppress: Option<ClientId>,
    ) {
        // `send` fails only when nobody is subscribed, which is normal.
        let _ = self.events.send(EventEnvelope {
            ev,
            json,
            revision,
            suppress,
        });
    }
}

impl SpawnDefaults {
    /// The [`SpawnSpec`] used for an auto-seeded Surface (fresh start, Q21
    /// re-seed): the configured shell in the fallback directory, pristine.
    #[must_use]
    pub fn seed_spec(&self) -> SpawnSpec {
        SpawnSpec {
            shell: self.shell.clone(),
            cwd: self.cwd.clone(),
            env: BTreeMap::new(),
            cols: self.cols,
            rows: self.rows,
            seeded: true,
        }
    }
}

/// Resolves a wire [`st_proto::SpawnSpec`] into the spawner's [`SpawnSpec`].
///
/// * `shell` falls back to `[shell]` from `config.toml` (`03-server.md` §9).
/// * `cwd` must be an absolute, existing directory (§10); otherwise the client
///   gets `bad_request` rather than a shell in a surprising place.
/// * `env` is filtered through the grilling-Q48 allow-list — the caller's
///   `env_allow` when it sent one, else [`st_proto::DEFAULT_ENV_ALLOW_LIST`].
pub fn resolve_spawn(
    spec: &st_proto::SpawnSpec,
    defaults: &SpawnDefaults,
    seeded: bool,
) -> Result<SpawnSpec, ErrorBody> {
    let shell = match &spec.shell {
        Some(argv) if !argv.is_empty() => argv.clone(),
        _ => defaults.shell.clone(),
    };

    let cwd = match &spec.cwd {
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(ErrorBody::new(
                    ErrorCode::BadRequest,
                    format!("cwd {} is not absolute", path.display()),
                ));
            }
            if !path.is_dir() {
                return Err(ErrorBody::new(
                    ErrorCode::BadRequest,
                    format!("cwd {} is not an existing directory", path.display()),
                ));
            }
            path
        }
        None => defaults.cwd.clone(),
    };

    let env = filter_env(spec.env.as_ref(), spec.env_allow.as_deref());

    Ok(SpawnSpec {
        shell,
        cwd,
        env,
        cols: if spec.cols == 0 {
            defaults.cols
        } else {
            spec.cols
        },
        rows: if spec.rows == 0 {
            defaults.rows
        } else {
            spec.rows
        },
        seeded,
    })
}

/// Applies the grilling-Q48 environment allow-list.
#[must_use]
pub fn filter_env(
    env: Option<&BTreeMap<String, String>>,
    allow: Option<&[String]>,
) -> BTreeMap<String, String> {
    let Some(env) = env else {
        return BTreeMap::new();
    };
    env.iter()
        .filter(|(name, _)| match allow {
            Some(list) => list.iter().any(|allowed| {
                if let Some(prefix) = allowed.strip_suffix('_') {
                    name.starts_with(prefix) && name.len() > prefix.len() + 1
                } else {
                    *name == allowed
                }
            }),
            None => st_proto::control::is_env_allowed(name),
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// The `workspace.get` result, with the `pid` extension.
///
/// `SurfaceMeta` (`02-protocol.md` §3.2) has no pid field, but `st ls` prints
/// one when the server offers it, and an extra field is a minor change (§10).
/// Serializing here rather than through `SurfaceMeta` keeps `st-proto` frozen.
#[must_use]
pub fn snapshot_value(ws: &Workspace) -> Value {
    let mut value = serde_json::to_value(ws.snapshot()).unwrap_or_else(|_| json!({}));
    inject_pids(&mut value, ws);
    value
}

/// The `ev.workspace` event and its wire form.
#[must_use]
pub fn workspace_event(ws: &Workspace) -> (Ev, Value) {
    let snapshot = ws.snapshot();
    let ev = Ev::Workspace {
        revision: ws.revision(),
        workspace: snapshot.workspace,
        surfaces: snapshot.surfaces,
    };
    let mut json = serde_json::to_value(&ev).unwrap_or_else(|_| json!({}));
    inject_pids(&mut json, ws);
    (ev, json)
}

fn inject_pids(value: &mut Value, ws: &Workspace) {
    let Some(surfaces) = value.get_mut("surfaces").and_then(Value::as_array_mut) else {
        return;
    };
    for entry in surfaces {
        let Some(object) = entry.as_object_mut() else {
            continue;
        };
        let Some(id) = object.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let pid = ws
            .surfaces
            .get(&SurfaceId(id as u32))
            .and_then(|s| match s.status {
                SurfaceStatus::Running { pid } => pid,
                SurfaceStatus::Exited { .. } => None,
            });
        if let Some(pid) = pid {
            object.insert("pid".to_string(), json!(pid));
        }
    }
}

fn value<T: serde::Serialize>(result: T) -> Result<Value, ErrorBody> {
    serde_json::to_value(result)
        .map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("cannot encode result: {e}")))
}

/// Re-seeds a Workspace from a loaded `workspace.json` (`03-server.md` §2.6).
///
/// Every saved Tab gets a *fresh* Surface in its saved cwd, falling back to the
/// default directory when it is gone. Ids for Sessions and Tabs are preserved;
/// Surface ids are re-allocated because the processes behind the old ones died
/// with the previous daemon. Every re-seeded Surface starts pristine
/// (grilling Q42).
pub fn reseed(
    file: &persist::WorkspaceFile,
    ws: &mut Workspace,
    spawner: &dyn SurfaceSpawner,
    defaults: &SpawnDefaults,
    metrics: &Metrics,
) {
    ws.set_next_id(file.next_id);

    for saved in &file.sessions {
        let mut tabs = Vec::new();
        for tab in &saved.tabs {
            let cwd = saved_cwd(tab.surface.cwd.as_deref(), defaults);
            let shell = if tab.surface.shell.is_empty() {
                defaults.shell.clone()
            } else {
                tab.surface.shell.clone()
            };
            let spec = SpawnSpec {
                shell,
                cwd,
                env: BTreeMap::new(),
                cols: defaults.cols,
                rows: defaults.rows,
                seeded: true,
            };
            match spawner.spawn(&spec) {
                Ok(spawned) => {
                    metrics.surfaces_spawned.inc();
                    ws.insert_surface(Surface {
                        id: spawned.id,
                        title: spawned.title.unwrap_or_else(|| tab.surface.title.clone()),
                        user_title: tab.surface.user_title.clone(),
                        cwd: Some(spec.cwd.display().to_string()),
                        shell: spec.shell.clone(),
                        cols: spec.cols,
                        rows: spec.rows,
                        has_foreground_child: false,
                        status: SurfaceStatus::Running { pid: spawned.pid },
                        view: st_proto::ViewState::default(),
                        pristine: true,
                    });
                    tabs.push(crate::workspace::model::Tab {
                        id: tab.id,
                        surface: spawned.id,
                    });
                }
                Err(e) => {
                    tracing::warn!(tab = %tab.id, error = %e, "cannot re-seed tab; dropping it");
                }
            }
        }

        if tabs.is_empty() {
            tracing::warn!(session = %saved.id, "no tab could be re-seeded; dropping the session");
            continue;
        }

        let active_tab = saved
            .active_tab
            .filter(|id| tabs.iter().any(|t| t.id == *id))
            .or_else(|| tabs.first().map(|t| t.id));

        ws.insert_session(crate::workspace::model::Session {
            id: saved.id,
            name: saved.name.clone(),
            active_tab,
            tabs,
        });
    }

    if ws.sessions.iter().any(|s| s.id == file.active_session) {
        ws.active_session = file.active_session;
    }
}

fn saved_cwd(cwd: Option<&str>, defaults: &SpawnDefaults) -> PathBuf {
    match cwd {
        Some(path) if Path::new(path).is_dir() => PathBuf::from(path),
        Some(path) => {
            tracing::info!(cwd = path, "saved cwd is gone; falling back to the default");
            defaults.cwd.clone()
        }
        None => defaults.cwd.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::DEFAULT_SESSION_NAME;
    use crate::workspace::spawn::NullSpawner;

    fn defaults() -> SpawnDefaults {
        SpawnDefaults {
            shell: vec!["/bin/zsh".into()],
            cwd: PathBuf::from("/tmp"),
            cols: 80,
            rows: 24,
        }
    }

    #[test]
    fn spawn_defaults_fill_in_an_empty_spec() {
        let resolved = resolve_spawn(&st_proto::SpawnSpec::default(), &defaults(), false).unwrap();
        assert_eq!(resolved.shell, vec!["/bin/zsh"]);
        assert_eq!(resolved.cwd, PathBuf::from("/tmp"));
        assert_eq!((resolved.cols, resolved.rows), (80, 24));
        assert!(!resolved.seeded);
    }

    #[test]
    fn a_relative_or_missing_cwd_is_a_bad_request() {
        let spec = st_proto::SpawnSpec {
            cwd: Some("relative/path".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_spawn(&spec, &defaults(), false).unwrap_err().code,
            ErrorCode::BadRequest
        );

        let spec = st_proto::SpawnSpec {
            cwd: Some("/definitely/not/here".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_spawn(&spec, &defaults(), false).unwrap_err().code,
            ErrorCode::BadRequest
        );
    }

    #[test]
    fn the_env_allow_list_is_applied() {
        let env: BTreeMap<String, String> = [
            ("PATH", "/usr/bin"),
            ("LC_ALL", "C"),
            ("AWS_SECRET_ACCESS_KEY", "hunter2"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

        let filtered = filter_env(Some(&env), None);
        assert_eq!(filtered.len(), 2, "grilling Q48 default allow-list");
        assert!(filtered.contains_key("PATH"));
        assert!(filtered.contains_key("LC_ALL"));

        let narrow = filter_env(Some(&env), Some(&["PATH".to_string()]));
        assert_eq!(narrow.keys().collect::<Vec<_>>(), vec!["PATH"]);
    }

    #[test]
    fn reseeding_recreates_the_shape_with_fresh_surfaces() {
        let mut original = Workspace::new();
        let spawner = NullSpawner::new();
        let metrics = Metrics::new();
        let spawned = spawner.spawn(&defaults().seed_spec()).unwrap();
        original.insert_surface(Surface {
            id: spawned.id,
            title: "zsh".into(),
            user_title: Some("keep me".into()),
            cwd: Some("/tmp".into()),
            shell: vec!["/bin/zsh".into()],
            cols: 80,
            rows: 24,
            has_foreground_child: false,
            status: SurfaceStatus::Running { pid: None },
            view: st_proto::ViewState::default(),
            pristine: true,
        });
        let (session, tab) = original.seed_default_session(spawned.id);
        let file = persist::snapshot_file(&original);

        let mut restored = Workspace::new();
        reseed(&file, &mut restored, &spawner, &defaults(), &metrics);

        assert_eq!(restored.sessions.len(), 1);
        assert_eq!(restored.sessions[0].id, session);
        assert_eq!(restored.sessions[0].name, DEFAULT_SESSION_NAME);
        assert_eq!(restored.sessions[0].tabs[0].id, tab);
        assert_ne!(
            restored.sessions[0].tabs[0].surface, spawned.id,
            "a re-seeded Surface is a new process and gets a new id"
        );
        let surface = restored
            .surface(restored.sessions[0].tabs[0].surface)
            .unwrap();
        assert!(surface.pristine, "grilling Q42");
        assert_eq!(surface.user_title.as_deref(), Some("keep me"));
        assert!(restored.next_id() > tab.get());
    }

    #[test]
    fn a_saved_cwd_that_vanished_falls_back() {
        assert_eq!(saved_cwd(Some("/tmp"), &defaults()), PathBuf::from("/tmp"));
        assert_eq!(
            saved_cwd(Some("/nope/nope"), &defaults()),
            PathBuf::from("/tmp")
        );
        assert_eq!(saved_cwd(None, &defaults()), PathBuf::from("/tmp"));
    }

    #[test]
    fn pids_are_injected_into_surface_metadata() {
        let mut ws = Workspace::new();
        ws.insert_surface(Surface {
            id: SurfaceId(1),
            title: "zsh".into(),
            user_title: None,
            cwd: None,
            shell: vec!["/bin/zsh".into()],
            cols: 80,
            rows: 24,
            has_foreground_child: false,
            status: SurfaceStatus::Running { pid: Some(4242) },
            view: st_proto::ViewState::default(),
            pristine: true,
        });
        ws.seed_default_session(SurfaceId(1));
        let value = snapshot_value(&ws);
        assert_eq!(value["surfaces"][0]["pid"], 4242);
    }
}
