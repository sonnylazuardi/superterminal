//! Control-plane messages — `docs/plan/02-protocol.md` §3.
//!
//! The control plane is newline-delimited JSON: exactly one message per
//! `\n`-terminated line, at most [`crate::frame::MAX_CONTROL_LINE`] bytes.
//! Every message is a JSON object with a `"t"` discriminant:
//!
//! * **Request** (client → server) — [`Req`], `{"t":"<name>","id":<u32>,…}`.
//! * **Response** — [`Res`], `{"t":"ok","id":n,"result":…}` or
//!   `{"t":"err","id":n,"error":{…}}`; exactly one per request, in any order.
//! * **Event** (server → client) — [`Ev`], `{"t":"ev.<name>",…}`, no `id`,
//!   sent only after `workspace.subscribe`.
//!
//! Mutating requests take an optional `if_revision`; when it is stale the
//! server answers [`ErrorCode::Conflict`] and the client re-reads. Unknown
//! fields are ignored on decode, which is what makes "add an optional field"
//! a minor change (§10).

use serde::{Deserialize, Serialize};

use crate::ids::{AbsLine, SessionId, SurfaceId, TabId};

/// Monotonic Workspace document revision; every successful mutation bumps it.
pub type Revision = u64;

/// Client-chosen request id, unique per connection while outstanding.
pub type ReqId = u32;

// ---------------------------------------------------------------- errors

/// Error codes of the `err` envelope (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Malformed message, unknown `t`, or a missing field.
    BadRequest,
    /// Unknown session/tab/surface id.
    NotFound,
    /// `if_revision` did not match the current Workspace revision.
    Conflict,
    /// The PTY or shell could not start; `message` carries the errno text.
    SpawnFailed,
    /// The message exists but not in the negotiated minor.
    Unsupported,
    /// The server is shutting down.
    ShuttingDown,
    /// Anything unexpected on the server side.
    Internal,
}

/// Body of an `err` response (§3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Machine-readable code.
    pub code: ErrorCode,
    /// Human-readable message.
    pub message: String,
    /// Optional structured detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ErrorBody {
    /// Builds an error body without extra data.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// A successful response: `{"t":"ok","id":n,"result":…}` (§3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename = "ok")]
pub struct OkRes<R> {
    /// The `id` of the request being answered.
    pub id: ReqId,
    /// The result, whose shape is fixed by the request (see [`Req`]).
    pub result: R,
}

/// A failed response: `{"t":"err","id":n,"error":{…}}` (§3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename = "err")]
pub struct ErrRes {
    /// The `id` of the request being answered.
    pub id: ReqId,
    /// What went wrong.
    pub error: ErrorBody,
}

/// Either half of the response envelope (§3.1).
///
/// `R` is the result type of the request being answered; [`AnyRes`] keeps it
/// as a raw JSON value for code that dispatches on the pending request's id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Res<R = serde_json::Value> {
    /// `{"t":"ok",…}`.
    #[serde(rename = "ok")]
    Ok {
        /// The `id` of the request being answered.
        id: ReqId,
        /// The result.
        result: R,
    },
    /// `{"t":"err",…}`.
    #[serde(rename = "err")]
    Err {
        /// The `id` of the request being answered.
        id: ReqId,
        /// What went wrong.
        error: ErrorBody,
    },
}

/// A response whose result has not been typed yet.
pub type AnyRes = Res<serde_json::Value>;

impl<R> Res<R> {
    /// The `id` of the request this answers.
    #[must_use]
    pub const fn id(&self) -> ReqId {
        match self {
            Res::Ok { id, .. } | Res::Err { id, .. } => *id,
        }
    }
}

// ---------------------------------------------------------------- workspace document

/// The Workspace document (§3.2). Pushed in full by [`Ev::Workspace`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Workspace {
    /// Increments on every change.
    pub revision: Revision,
    /// The Session the client should show.
    pub active_session: SessionId,
    /// Sessions, in display order.
    pub sessions: Vec<Session>,
}

/// A Session: a named, ordered group of Tabs (§3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Session id.
    pub id: SessionId,
    /// Display name; the first Session is called `Default` (grilling Q48).
    pub name: String,
    /// The Tab to show when this Session becomes active; persisted (Q48).
    pub active_tab: Option<TabId>,
    /// Tabs, in display order.
    pub tabs: Vec<Tab>,
}

/// A Tab. In v1 a Tab holds exactly one Surface (grilling Q19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    /// Tab id.
    pub id: TabId,
    /// The Surface shown in this Tab.
    pub surface: SurfaceId,
}

/// Everything the chrome needs to know about a Surface (§3.2).
///
/// `cwd` and `has_foreground_child` are the Q48 additions the tab-close
/// confirmation dialog needs; the server samples the PTY's foreground process
/// group to fill them in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceMeta {
    /// Surface id.
    pub id: SurfaceId,
    /// Title as set by the program (OSC 0/2).
    pub title: String,
    /// Title set by the user through `surface.rename`, which wins in the UI.
    pub user_title: Option<String>,
    /// Current working directory of the foreground process, when known.
    pub cwd: Option<String>,
    /// Grid width.
    pub cols: u16,
    /// Grid height.
    pub rows: u16,
    /// Whether a program other than the shell is in the foreground
    /// (grilling Q48); the client uses it to confirm before closing.
    pub has_foreground_child: bool,
    /// Running or exited.
    pub state: SurfaceState,
    /// Scroll offset and selection, persisted server-side (grilling Q17).
    pub view_state: ViewState,
}

/// Whether a Surface's process is still alive (§3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceState {
    /// The process is running.
    Running,
    /// The process ended; the Tab shows "press Enter to close" (grilling Q22).
    Exited {
        /// Exit code, when it exited normally.
        code: Option<i32>,
        /// Signal name (e.g. `"SIGKILL"`), when it was killed.
        signal: Option<String>,
    },
}

/// A Surface's view state (§3.2). Also carried by `Snapshot` on the data plane.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ViewState {
    /// Lines above the bottom; `0` = following output.
    pub scroll_offset: u32,
    /// The current selection, if any. Cleared by a resize (grilling Q40).
    pub selection: Option<Selection>,
}

/// A selection, in absolute line coordinates so it survives scrolling (§3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    /// Selection shape.
    pub kind: SelectionKind,
    /// Where the drag started.
    pub anchor: AbsPoint,
    /// Where the pointer is now.
    pub head: AbsPoint,
}

/// Selection shapes (§3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionKind {
    /// Flowing text selection.
    #[default]
    Normal,
    /// Rectangular block selection.
    Block,
    /// Whole lines.
    Lines,
}

/// A point in a Surface's absolute coordinate space (§3.2, §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct AbsPoint {
    /// Absolute line id, so the point survives scrolling and trimming.
    pub line: AbsLine,
    /// Column within that line.
    pub col: u16,
}

/// `workspace.get` / `workspace.subscribe` result (§3.2).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// The document.
    pub workspace: Workspace,
    /// Metadata for every Surface it mentions.
    pub surfaces: Vec<SurfaceMeta>,
}

// ---------------------------------------------------------------- requests

/// Environment variables a client may ask to be forwarded from *its* process
/// into a freshly spawned shell (grilling Q48).
///
/// The daemon's own environment is frozen at first spawn, so without this a
/// long-lived server hands out a stale `DISPLAY`, `SSH_AUTH_SOCK`, and so on.
/// The server applies this list as an allow-list over
/// [`SpawnSpec::env`]; names it does not recognise are dropped. `LC_*` is
/// matched as a prefix.
pub const DEFAULT_ENV_ALLOW_LIST: &[&str] = &[
    "PATH",
    "LANG",
    "LC_",
    "SSH_AUTH_SOCK",
    "DISPLAY",
    "WAYLAND_DISPLAY",
];

/// Returns `true` when `name` is permitted by [`DEFAULT_ENV_ALLOW_LIST`]
/// (`LC_*` matches as a prefix).
#[must_use]
pub fn is_env_allowed(name: &str) -> bool {
    DEFAULT_ENV_ALLOW_LIST.iter().any(|allowed| {
        if allowed.ends_with('_') {
            name.starts_with(allowed) && name.len() > allowed.len()
        } else {
            name == *allowed
        }
    })
}

/// Deserializes a tri-state JSON field: absent → `None`, `null` → `Some(None)`,
/// a value → `Some(Some(v))`. Needed because plain `Option<Option<T>>` collapses
/// `null` to `None` (§3.3, `view.set`).
fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::deserialize(deserializer).map(Some)
}

/// How to spawn a Surface's process (§3.3).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SpawnSpec {
    /// argv; defaults to `config.toml`'s shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<Vec<String>>,
    /// Working directory; defaults to the config value or `$HOME`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Variables merged over the server's environment, filtered by
    /// `env_allow` (grilling Q48).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    /// Allow-list restricting `env`; when absent the server applies
    /// [`DEFAULT_ENV_ALLOW_LIST`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_allow: Option<Vec<String>>,
    /// Initial column count.
    pub cols: u16,
    /// Initial row count.
    pub rows: u16,
}

/// Signals `surface.kill` accepts (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum KillSignal {
    /// `SIGHUP`.
    Hup,
    /// `SIGTERM`, the default.
    #[default]
    Term,
    /// `SIGKILL`.
    Kill,
}

/// Every client → server request (§3.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Req {
    /// Read the Workspace document. Result: [`WorkspaceSnapshot`].
    #[serde(rename = "workspace.get")]
    WorkspaceGet {
        /// Request id.
        id: ReqId,
    },
    /// Read it and subscribe to [`Ev`]. Result: [`WorkspaceSnapshot`].
    #[serde(rename = "workspace.subscribe")]
    WorkspaceSubscribe {
        /// Request id.
        id: ReqId,
    },
    /// Create a Session. Result: [`SessionCreated`].
    #[serde(rename = "session.create")]
    SessionCreate {
        /// Request id.
        id: ReqId,
        /// Display name.
        name: String,
        /// Optimistic concurrency guard.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        if_revision: Option<Revision>,
    },
    /// Rename a Session. Result: [`RevisionResult`].
    #[serde(rename = "session.rename")]
    SessionRename {
        /// Request id.
        id: ReqId,
        /// Target Session.
        session: SessionId,
        /// New name.
        name: String,
        /// Optimistic concurrency guard.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        if_revision: Option<Revision>,
    },
    /// Delete a Session and kill its Surfaces (grilling Q21).
    /// Result: [`RevisionResult`].
    #[serde(rename = "session.delete")]
    SessionDelete {
        /// Request id.
        id: ReqId,
        /// Target Session.
        session: SessionId,
        /// Optimistic concurrency guard.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        if_revision: Option<Revision>,
    },
    /// List Sessions. Result: [`SessionList`].
    #[serde(rename = "session.list")]
    SessionList {
        /// Request id.
        id: ReqId,
    },
    /// Switch the active Session. Result: [`RevisionResult`].
    #[serde(rename = "session.set_active")]
    SessionSetActive {
        /// Request id.
        id: ReqId,
        /// Target Session.
        session: SessionId,
    },
    /// Create a Tab, spawning a Surface or adopting an existing one — exactly
    /// one of `spawn` and `surface`. Result: [`TabCreated`].
    #[serde(rename = "tab.create")]
    TabCreate {
        /// Request id.
        id: ReqId,
        /// Session to create the Tab in.
        session: SessionId,
        /// Insertion index; appended when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<u32>,
        /// Spawn a new Surface.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawn: Option<SpawnSpec>,
        /// Adopt this detached Surface instead.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        surface: Option<SurfaceId>,
        /// Optimistic concurrency guard.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        if_revision: Option<Revision>,
    },
    /// Close a Tab, killing its Surface (grilling Q21). Result: [`RevisionResult`].
    #[serde(rename = "tab.close")]
    TabClose {
        /// Request id.
        id: ReqId,
        /// Target Tab.
        tab: TabId,
        /// Optimistic concurrency guard.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        if_revision: Option<Revision>,
    },
    /// Move a Tab within its Session. Result: [`RevisionResult`].
    #[serde(rename = "tab.reorder")]
    TabReorder {
        /// Request id.
        id: ReqId,
        /// Target Tab.
        tab: TabId,
        /// New index within the Session.
        index: u32,
        /// Optimistic concurrency guard.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        if_revision: Option<Revision>,
    },
    /// Move a Tab to another Session. Result: [`RevisionResult`].
    #[serde(rename = "tab.move")]
    TabMove {
        /// Request id.
        id: ReqId,
        /// Target Tab.
        tab: TabId,
        /// Destination Session.
        to_session: SessionId,
        /// Insertion index; appended when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<u32>,
        /// Optimistic concurrency guard.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        if_revision: Option<Revision>,
    },
    /// Make a Tab its Session's active one (grilling Q48).
    /// Result: [`RevisionResult`].
    #[serde(rename = "tab.set_active")]
    TabSetActive {
        /// Request id.
        id: ReqId,
        /// Target Tab.
        tab: TabId,
    },
    /// Spawn a detached Surface that `tab.create` can adopt.
    /// Result: [`SurfaceCreated`].
    #[serde(rename = "surface.create")]
    SurfaceCreate {
        /// Request id.
        id: ReqId,
        /// How to spawn it.
        spawn: SpawnSpec,
    },
    /// Signal a Surface's process. Result: [`Empty`].
    #[serde(rename = "surface.kill")]
    SurfaceKill {
        /// Request id.
        id: ReqId,
        /// Target Surface.
        surface: SurfaceId,
        /// Signal to send; `TERM` when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<KillSignal>,
    },
    /// Set or clear a Surface's user title. Result: [`RevisionResult`].
    #[serde(rename = "surface.rename")]
    SurfaceRename {
        /// Request id.
        id: ReqId,
        /// Target Surface.
        surface: SurfaceId,
        /// The new user title; `null` clears it.
        user_title: Option<String>,
    },
    /// Persist scroll offset and/or selection (grilling Q17, Q24).
    ///
    /// Both fields are tri-state: absent leaves the value alone, `null`
    /// clears the selection. Result: [`RevisionResult`].
    #[serde(rename = "view.set")]
    ViewSet {
        /// Request id.
        id: ReqId,
        /// Target Surface.
        surface: SurfaceId,
        /// New scroll offset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scroll_offset: Option<u32>,
        /// New selection; `Some(None)` clears it.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "double_option"
        )]
        selection: Option<Option<Selection>>,
    },
    /// Ask for server diagnostics. Result: [`ServerStatus`].
    #[serde(rename = "server.status")]
    ServerStatus {
        /// Request id.
        id: ReqId,
    },
    /// Shut the daemon down; refused while Surfaces exist unless `force`.
    /// Result: [`Empty`].
    #[serde(rename = "server.shutdown")]
    ServerShutdown {
        /// Request id.
        id: ReqId,
        /// Kill running Surfaces instead of refusing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        force: Option<bool>,
    },
}

impl Req {
    /// The `id` this request must be answered with.
    #[must_use]
    pub const fn id(&self) -> ReqId {
        match self {
            Req::WorkspaceGet { id }
            | Req::WorkspaceSubscribe { id }
            | Req::SessionCreate { id, .. }
            | Req::SessionRename { id, .. }
            | Req::SessionDelete { id, .. }
            | Req::SessionList { id }
            | Req::SessionSetActive { id, .. }
            | Req::TabCreate { id, .. }
            | Req::TabClose { id, .. }
            | Req::TabReorder { id, .. }
            | Req::TabMove { id, .. }
            | Req::TabSetActive { id, .. }
            | Req::SurfaceCreate { id, .. }
            | Req::SurfaceKill { id, .. }
            | Req::SurfaceRename { id, .. }
            | Req::ViewSet { id, .. }
            | Req::ServerStatus { id }
            | Req::ServerShutdown { id, .. } => *id,
        }
    }

    /// The `if_revision` guard, when this request carries one (§3.2).
    #[must_use]
    pub const fn if_revision(&self) -> Option<Revision> {
        match self {
            Req::SessionCreate { if_revision, .. }
            | Req::SessionRename { if_revision, .. }
            | Req::SessionDelete { if_revision, .. }
            | Req::TabCreate { if_revision, .. }
            | Req::TabClose { if_revision, .. }
            | Req::TabReorder { if_revision, .. }
            | Req::TabMove { if_revision, .. } => *if_revision,
            _ => None,
        }
    }

    /// The `"t"` discriminant, as it appears on the wire.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Req::WorkspaceGet { .. } => "workspace.get",
            Req::WorkspaceSubscribe { .. } => "workspace.subscribe",
            Req::SessionCreate { .. } => "session.create",
            Req::SessionRename { .. } => "session.rename",
            Req::SessionDelete { .. } => "session.delete",
            Req::SessionList { .. } => "session.list",
            Req::SessionSetActive { .. } => "session.set_active",
            Req::TabCreate { .. } => "tab.create",
            Req::TabClose { .. } => "tab.close",
            Req::TabReorder { .. } => "tab.reorder",
            Req::TabMove { .. } => "tab.move",
            Req::TabSetActive { .. } => "tab.set_active",
            Req::SurfaceCreate { .. } => "surface.create",
            Req::SurfaceKill { .. } => "surface.kill",
            Req::SurfaceRename { .. } => "surface.rename",
            Req::ViewSet { .. } => "view.set",
            Req::ServerStatus { .. } => "server.status",
            Req::ServerShutdown { .. } => "server.shutdown",
        }
    }
}

// ---------------------------------------------------------------- results

/// Result of a request that only bumps the Workspace revision (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RevisionResult {
    /// The new Workspace revision.
    pub revision: Revision,
}

/// Result of `session.create` (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCreated {
    /// The new Session.
    pub session: SessionId,
    /// The new Workspace revision.
    pub revision: Revision,
}

/// Result of `session.list` (§3.3).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionList {
    /// All Sessions, in order.
    pub sessions: Vec<Session>,
}

/// Result of `tab.create` (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabCreated {
    /// The new Tab.
    pub tab: TabId,
    /// The Surface it holds (spawned or adopted).
    pub surface: SurfaceId,
    /// The new Workspace revision.
    pub revision: Revision,
}

/// Result of `surface.create` (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceCreated {
    /// The new, still detached Surface.
    pub surface: SurfaceId,
}

/// Result of a request that returns `{}` (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Empty {}

/// Result of `server.status` (§3.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerStatus {
    /// The daemon's build id.
    pub build_id: String,
    /// The negotiated protocol version, as `"major.minor"`.
    pub proto_version: String,
    /// The daemon's pid.
    pub pid: u32,
    /// Seconds since the daemon started.
    pub uptime_s: u64,
    /// Number of live Surfaces.
    pub surfaces: u32,
    /// Number of connected control-plane clients.
    pub control_clients: u32,
    /// Number of connected data-plane clients.
    pub data_clients: u32,
    /// Path of the persisted Workspace document (grilling Q18).
    pub workspace_file: String,
}

// ---------------------------------------------------------------- events

/// Unsolicited server → client messages, sent after `workspace.subscribe` (§3.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Ev {
    /// The whole Workspace document changed. It is a few KB, so there are no
    /// fine-grained patch events in 1.0 (§12, gap 8).
    #[serde(rename = "ev.workspace")]
    Workspace {
        /// The new revision, equal to `workspace.revision`.
        revision: Revision,
        /// The document.
        workspace: Workspace,
        /// Metadata for every Surface it mentions.
        surfaces: Vec<SurfaceMeta>,
    },
    /// A Surface's process ended.
    #[serde(rename = "ev.surface_exited")]
    SurfaceExited {
        /// The Surface that exited.
        surface: SurfaceId,
        /// Exit code, when it exited normally.
        code: Option<i32>,
        /// Signal name, when it was killed.
        signal: Option<String>,
    },
    /// The daemon is going away; clients should show a banner (grilling Q31).
    #[serde(rename = "ev.server_shutting_down")]
    ServerShuttingDown {
        /// Human-readable reason.
        reason: String,
    },
}

// ---------------------------------------------------------------- handshake + union

/// The handshake messages, in their control-plane (JSON) framing (§2).
///
/// On the data plane the same three structs travel bare, under
/// `msg_type` `0x0001`–`0x0003`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Handshake {
    /// `{"t":"hello",…}`.
    #[serde(rename = "hello")]
    Hello(crate::frame::Hello),
    /// `{"t":"hello.ack",…}`.
    #[serde(rename = "hello.ack")]
    HelloAck(crate::frame::HelloAck),
    /// `{"t":"reject",…}`.
    #[serde(rename = "reject")]
    Reject(crate::frame::Reject),
}

/// Any control-plane message (§3.3, `ControlMsg`).
///
/// Deserialization tries the variants in order; each inner type rejects a
/// `"t"` it does not know, so the union is unambiguous.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ControlMsg {
    /// A handshake message.
    Handshake(Handshake),
    /// A request.
    Req(Box<Req>),
    /// A response.
    Res(AnyRes),
    /// An event.
    Ev(Box<Ev>),
}

impl ControlMsg {
    /// Parses one NDJSON line.
    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }

    /// Serializes to one NDJSON line, *without* the trailing `\n`.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_workspace() -> Workspace {
        Workspace {
            revision: 42,
            active_session: SessionId(1),
            sessions: vec![Session {
                id: SessionId(1),
                name: "Default".into(),
                active_tab: Some(TabId(12)),
                tabs: vec![Tab {
                    id: TabId(12),
                    surface: SurfaceId(9),
                }],
            }],
        }
    }

    fn sample_surface_meta() -> SurfaceMeta {
        SurfaceMeta {
            id: SurfaceId(9),
            title: "zsh".into(),
            user_title: None,
            cwd: Some("/home/sonny".into()),
            cols: 200,
            rows: 60,
            has_foreground_child: true,
            state: SurfaceState::Exited {
                code: Some(0),
                signal: None,
            },
            view_state: ViewState {
                scroll_offset: 3,
                selection: Some(Selection {
                    kind: SelectionKind::Lines,
                    anchor: AbsPoint {
                        line: AbsLine(10342),
                        col: 0,
                    },
                    head: AbsPoint {
                        line: AbsLine(10343),
                        col: 17,
                    },
                }),
            },
        }
    }

    pub(crate) fn every_request() -> Vec<Req> {
        vec![
            Req::WorkspaceGet { id: 1 },
            Req::WorkspaceSubscribe { id: 2 },
            Req::SessionCreate {
                id: 3,
                name: "Default".into(),
                if_revision: Some(41),
            },
            Req::SessionRename {
                id: 4,
                session: SessionId(1),
                name: "work".into(),
                if_revision: None,
            },
            Req::SessionDelete {
                id: 5,
                session: SessionId(1),
                if_revision: Some(41),
            },
            Req::SessionList { id: 6 },
            Req::SessionSetActive {
                id: 7,
                session: SessionId(1),
            },
            Req::TabCreate {
                id: 8,
                session: SessionId(1),
                index: Some(0),
                spawn: Some(SpawnSpec {
                    shell: Some(vec!["/bin/zsh".into(), "-l".into()]),
                    cwd: Some("/home/sonny/projects/superterminal".into()),
                    env: Some(
                        [("LANG".to_string(), "C.UTF-8".to_string())]
                            .into_iter()
                            .collect(),
                    ),
                    env_allow: Some(vec!["LANG".into()]),
                    cols: 200,
                    rows: 60,
                }),
                surface: None,
                if_revision: Some(41),
            },
            Req::TabClose {
                id: 9,
                tab: TabId(12),
                if_revision: None,
            },
            Req::TabReorder {
                id: 10,
                tab: TabId(12),
                index: 2,
                if_revision: Some(43),
            },
            Req::TabMove {
                id: 11,
                tab: TabId(12),
                to_session: SessionId(2),
                index: None,
                if_revision: Some(44),
            },
            Req::TabSetActive {
                id: 12,
                tab: TabId(12),
            },
            Req::SurfaceCreate {
                id: 13,
                spawn: SpawnSpec {
                    cols: 80,
                    rows: 24,
                    ..SpawnSpec::default()
                },
            },
            Req::SurfaceKill {
                id: 14,
                surface: SurfaceId(9),
                signal: Some(KillSignal::Kill),
            },
            Req::SurfaceRename {
                id: 15,
                surface: SurfaceId(9),
                user_title: None,
            },
            Req::ViewSet {
                id: 16,
                surface: SurfaceId(9),
                scroll_offset: Some(0),
                selection: Some(None),
            },
            Req::ServerStatus { id: 17 },
            Req::ServerShutdown {
                id: 18,
                force: Some(true),
            },
        ]
    }

    pub(crate) fn every_event() -> Vec<Ev> {
        vec![
            Ev::Workspace {
                revision: 42,
                workspace: sample_workspace(),
                surfaces: vec![sample_surface_meta()],
            },
            Ev::SurfaceExited {
                surface: SurfaceId(9),
                code: None,
                signal: Some("SIGKILL".into()),
            },
            Ev::ServerShuttingDown {
                reason: "idle".into(),
            },
        ]
    }

    #[test]
    fn every_request_round_trips_through_json() {
        for req in every_request() {
            let line = serde_json::to_string(&req).unwrap();
            assert_eq!(serde_json::from_str::<Req>(&line).unwrap(), req);
            let tag = serde_json::from_str::<serde_json::Value>(&line).unwrap()["t"]
                .as_str()
                .unwrap()
                .to_string();
            assert_eq!(tag, req.tag());
        }
    }

    #[test]
    fn every_event_round_trips_through_json() {
        for ev in every_event() {
            let line = serde_json::to_string(&ev).unwrap();
            assert_eq!(serde_json::from_str::<Ev>(&line).unwrap(), ev);
            assert!(line.contains("\"t\":\"ev."));
        }
    }

    #[test]
    fn request_ids_and_revision_guards() {
        for (i, req) in every_request().iter().enumerate() {
            assert_eq!(req.id() as usize, i + 1);
        }
        assert_eq!(
            Req::TabClose {
                id: 1,
                tab: TabId(1),
                if_revision: Some(7)
            }
            .if_revision(),
            Some(7)
        );
        assert_eq!(Req::WorkspaceGet { id: 1 }.if_revision(), None);
    }

    #[test]
    fn tab_create_matches_the_spec_example() {
        let line = r#"{"t":"tab.create","id":7,"session":1,"spawn":{"cwd":"/home/sonny/projects/superterminal","cols":200,"rows":60},"if_revision":41}"#;
        let req: Req = serde_json::from_str(line).unwrap();
        let Req::TabCreate {
            id,
            session,
            spawn,
            if_revision,
            index,
            surface,
        } = req
        else {
            panic!("expected tab.create");
        };
        assert_eq!(id, 7);
        assert_eq!(session, SessionId(1));
        assert_eq!(if_revision, Some(41));
        assert_eq!(index, None);
        assert_eq!(surface, None);
        let spawn = spawn.unwrap();
        assert_eq!(spawn.cols, 200);
        assert_eq!(
            spawn.cwd.as_deref(),
            Some("/home/sonny/projects/superterminal")
        );
        assert_eq!(spawn.shell, None);
    }

    #[test]
    fn view_set_matches_the_spec_example() {
        let line = r#"{"t":"view.set","id":8,"surface":9,"selection":{"kind":"normal","anchor":{"line":10342,"col":0},"head":{"line":10343,"col":17}}}"#;
        let req: Req = serde_json::from_str(line).unwrap();
        let Req::ViewSet {
            scroll_offset,
            selection,
            ..
        } = req
        else {
            panic!("expected view.set");
        };
        assert_eq!(scroll_offset, None, "absent means 'leave alone'");
        assert_eq!(
            selection.unwrap().unwrap().head,
            AbsPoint {
                line: AbsLine(10343),
                col: 17
            }
        );
    }

    #[test]
    fn view_set_distinguishes_absent_from_null_selection() {
        let absent: Req = serde_json::from_str(r#"{"t":"view.set","id":1,"surface":9}"#).unwrap();
        let cleared: Req =
            serde_json::from_str(r#"{"t":"view.set","id":1,"surface":9,"selection":null}"#)
                .unwrap();
        let Req::ViewSet { selection: a, .. } = absent else {
            unreachable!()
        };
        let Req::ViewSet { selection: b, .. } = cleared else {
            unreachable!()
        };
        assert_eq!(a, None);
        assert_eq!(b, Some(None));
    }

    #[test]
    fn ok_and_err_envelopes() {
        let ok = OkRes {
            id: 7,
            result: TabCreated {
                tab: TabId(12),
                surface: SurfaceId(9),
                revision: 42,
            },
        };
        assert_eq!(
            serde_json::to_value(&ok).unwrap(),
            json!({"t":"ok","id":7,"result":{"tab":12,"surface":9,"revision":42}})
        );
        assert_eq!(
            serde_json::from_str::<OkRes<TabCreated>>(
                r#"{"t":"ok","id":7,"result":{"tab":12,"surface":9,"revision":42}}"#
            )
            .unwrap(),
            ok
        );

        let err = ErrRes {
            id: 9,
            error: ErrorBody::new(ErrorCode::NotFound, "tab 999 does not exist"),
        };
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({"t":"err","id":9,"error":{"code":"not_found","message":"tab 999 does not exist"}})
        );
    }

    #[test]
    fn res_enum_round_trips_both_arms() {
        let ok: Res<RevisionResult> = Res::Ok {
            id: 8,
            result: RevisionResult { revision: 43 },
        };
        let line = serde_json::to_string(&ok).unwrap();
        assert_eq!(line, r#"{"t":"ok","id":8,"result":{"revision":43}}"#);
        assert_eq!(
            serde_json::from_str::<Res<RevisionResult>>(&line).unwrap(),
            ok
        );
        assert_eq!(ok.id(), 8);

        let err: AnyRes = Res::Err {
            id: 8,
            error: ErrorBody {
                code: ErrorCode::Conflict,
                message: "stale".into(),
                data: Some(json!({"revision": 44})),
            },
        };
        let line = serde_json::to_string(&err).unwrap();
        assert_eq!(serde_json::from_str::<AnyRes>(&line).unwrap(), err);
    }

    #[test]
    fn all_error_codes_render_as_snake_case() {
        let codes = [
            (ErrorCode::BadRequest, "bad_request"),
            (ErrorCode::NotFound, "not_found"),
            (ErrorCode::Conflict, "conflict"),
            (ErrorCode::SpawnFailed, "spawn_failed"),
            (ErrorCode::Unsupported, "unsupported"),
            (ErrorCode::ShuttingDown, "shutting_down"),
            (ErrorCode::Internal, "internal"),
        ];
        for (code, text) in codes {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(text));
            assert_eq!(
                serde_json::from_value::<ErrorCode>(json!(text)).unwrap(),
                code
            );
        }
    }

    #[test]
    fn workspace_document_json_shape() {
        let snapshot = WorkspaceSnapshot {
            workspace: sample_workspace(),
            surfaces: vec![sample_surface_meta()],
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["workspace"]["sessions"][0]["name"], json!("Default"));
        assert_eq!(value["workspace"]["sessions"][0]["active_tab"], json!(12));
        assert_eq!(value["surfaces"][0]["state"]["kind"], json!("exited"));
        assert_eq!(value["surfaces"][0]["has_foreground_child"], json!(true));
        assert_eq!(
            value["surfaces"][0]["view_state"]["selection"]["kind"],
            json!("lines")
        );
        assert_eq!(
            serde_json::from_value::<WorkspaceSnapshot>(value).unwrap(),
            snapshot
        );
    }

    #[test]
    fn running_surface_state_is_a_bare_kind() {
        assert_eq!(
            serde_json::to_value(SurfaceState::Running).unwrap(),
            json!({"kind":"running"})
        );
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let req: Req = serde_json::from_str(
            r#"{"t":"tab.close","id":9,"tab":999,"colour":"red","future":{"a":1}}"#,
        )
        .unwrap();
        assert_eq!(req.id(), 9);
    }

    #[test]
    fn unknown_request_tag_is_rejected() {
        assert!(serde_json::from_str::<Req>(r#"{"t":"tab.explode","id":1}"#).is_err());
    }

    #[test]
    fn control_msg_union_dispatches_on_the_tag() {
        let cases: Vec<(&str, fn(&ControlMsg) -> bool)> = vec![
            (
                r#"{"t":"hello","proto_version":"1.0","client_kind":"tool","build_id":"x"}"#,
                |m| matches!(m, ControlMsg::Handshake(Handshake::Hello(_))),
            ),
            (
                r#"{"t":"hello.ack","proto_version":"1.0","server_build_id":"x","workspace_revision":1,"server_pid":2}"#,
                |m| matches!(m, ControlMsg::Handshake(Handshake::HelloAck(_))),
            ),
            (
                r#"{"t":"reject","reason":"major_mismatch","message":"m","server_version":"1.0"}"#,
                |m| matches!(m, ControlMsg::Handshake(Handshake::Reject(_))),
            ),
            (r#"{"t":"workspace.get","id":1}"#, |m| {
                matches!(m, ControlMsg::Req(_))
            }),
            (r#"{"t":"ok","id":1,"result":{}}"#, |m| {
                matches!(m, ControlMsg::Res(Res::Ok { .. }))
            }),
            (
                r#"{"t":"err","id":1,"error":{"code":"internal","message":"m"}}"#,
                |m| matches!(m, ControlMsg::Res(Res::Err { .. })),
            ),
            (r#"{"t":"ev.server_shutting_down","reason":"idle"}"#, |m| {
                matches!(m, ControlMsg::Ev(_))
            }),
        ];
        for (line, check) in cases {
            let msg = ControlMsg::from_line(line).unwrap();
            assert!(check(&msg), "wrong variant for {line}");
            // Re-serializing produces a line that parses to the same message.
            let again = ControlMsg::from_line(&msg.to_line().unwrap()).unwrap();
            assert_eq!(again, msg);
        }
        assert!(ControlMsg::from_line(r#"{"t":"nope"}"#).is_err());
    }

    #[test]
    fn env_allow_list() {
        assert!(is_env_allowed("PATH"));
        assert!(is_env_allowed("LC_ALL"));
        assert!(is_env_allowed("WAYLAND_DISPLAY"));
        assert!(!is_env_allowed("LC_"));
        assert!(!is_env_allowed("LD_PRELOAD"));
        assert!(!is_env_allowed("PATHX"));
    }

    #[test]
    fn kill_signal_names() {
        assert_eq!(serde_json::to_value(KillSignal::Hup).unwrap(), json!("HUP"));
        assert_eq!(
            serde_json::to_value(KillSignal::default()).unwrap(),
            json!("TERM")
        );
    }
}
