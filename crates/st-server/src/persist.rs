//! `workspace.json` — the persisted Workspace *shape* (`03-server.md` §8).
//!
//! What is written: the version, a timestamp, the id counter, the active
//! Session, and for every Session its Tabs with each Surface's `cwd`, `shell`,
//! `title` and `user_title`.
//!
//! What is **not** written (§8): grid contents and scrollback, `ViewState`
//! (meaningless for a fresh shell), pids, `SurfaceStatus`, seq counters, style
//! tables, connection state and terminal size. A restart therefore recreates
//! the *shape*, never the history — grilling Q18.
//!
//! Writing is a 500 ms trailing debounce driven by [`Persister`], each write is
//! `workspace.json.tmp` → `fsync` → `rename`, and `SIGTERM` flushes
//! immediately. A file that will not parse, or carries an unknown `version`,
//! is renamed to `workspace.json.bad` and the daemon starts fresh with a
//! warning.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use st_proto::{SessionId, SurfaceId, TabId};
use tokio::sync::{mpsc, oneshot};

use crate::metrics::Metrics;
use crate::workspace::model::{Session, Surface, Workspace};

/// The only `version` this daemon understands.
pub const WORKSPACE_FILE_VERSION: u32 = 1;

/// The trailing debounce applied to structural changes (§8).
pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// `workspace.json`, version 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceFile {
    /// Schema version. Anything but [`WORKSPACE_FILE_VERSION`] is treated as
    /// corrupt (§8).
    pub version: u32,
    /// RFC 3339 UTC timestamp of the write; informational only.
    pub saved_at: String,
    /// The Session/Tab id counter, so ids are not reused across restarts.
    pub next_id: u32,
    /// The Session that was active.
    pub active_session: SessionId,
    /// The Sessions, in display order.
    pub sessions: Vec<PersistedSession>,
}

/// A Session as persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSession {
    /// Session id, preserved across restarts.
    pub id: SessionId,
    /// Display name.
    pub name: String,
    /// The Tab that was active, persisted per grilling Q48.
    #[serde(default)]
    pub active_tab: Option<TabId>,
    /// Tabs, in display order.
    pub tabs: Vec<PersistedTab>,
}

/// A Tab as persisted, with its Surface inlined (§8's example shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedTab {
    /// Tab id, preserved across restarts.
    pub id: TabId,
    /// The Surface to recreate for this Tab.
    pub surface: PersistedSurface,
}

/// The re-seed recipe for one Surface.
///
/// **Deviation from `03-server.md` §8:** `shell` is the full argv rather than a
/// single path string, because `SpawnSpec.shell` is `string[]` on the wire
/// (`02-protocol.md` §3.3) and a Surface spawned with `["/bin/zsh", "-l"]`
/// must come back the same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSurface {
    /// The id the Surface had. Informational: a re-seed allocates a fresh id
    /// from the spawner, since the process behind the old id is gone.
    pub id: SurfaceId,
    /// Where to start the new shell; falls back to `$HOME` when it is gone.
    pub cwd: Option<String>,
    /// argv of the shell to start.
    pub shell: Vec<String>,
    /// The last title seen, shown until the new shell sets its own.
    pub title: String,
    /// A `surface.rename` title, which survives the restart because the user
    /// chose it.
    #[serde(default)]
    pub user_title: Option<String>,
}

impl WorkspaceFile {
    /// Projects the live Workspace into the persisted shape.
    ///
    /// Detached Surfaces (`surface.create` with no Tab) are dropped: there is
    /// nothing in the document to hang them off after a restart.
    #[must_use]
    pub fn from_workspace(ws: &Workspace) -> Self {
        Self {
            version: WORKSPACE_FILE_VERSION,
            saved_at: rfc3339(SystemTime::now()),
            next_id: ws.next_id(),
            active_session: ws.active_session,
            sessions: ws.sessions.iter().map(persist_session).collect(),
        }
    }

    /// Serializes to the exact bytes written to disk (pretty, newline-ended).
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

fn persist_session(session: &Session) -> PersistedSession {
    PersistedSession {
        id: session.id,
        name: session.name.clone(),
        active_tab: session.active_tab,
        tabs: Vec::new(),
    }
}

/// Builds the persisted form, resolving each Tab's Surface out of the
/// Workspace's Surface table.
#[must_use]
pub fn snapshot_file(ws: &Workspace) -> WorkspaceFile {
    let mut file = WorkspaceFile::from_workspace(ws);
    for (session, persisted) in ws.sessions.iter().zip(file.sessions.iter_mut()) {
        persisted.tabs = session
            .tabs
            .iter()
            .filter_map(|tab| {
                ws.surfaces.get(&tab.surface).map(|surface| PersistedTab {
                    id: tab.id,
                    surface: persist_surface(surface),
                })
            })
            .collect();
    }
    file
}

fn persist_surface(surface: &Surface) -> PersistedSurface {
    PersistedSurface {
        id: surface.id,
        cwd: surface.cwd.clone(),
        shell: surface.shell.clone(),
        title: surface.title.clone(),
        user_title: surface.user_title.clone(),
    }
}

/// What [`load`] found on disk.
#[derive(Debug)]
pub enum Loaded {
    /// No file yet: a first run, or a state directory that was wiped.
    Missing,
    /// A valid version-1 document.
    File(Box<WorkspaceFile>),
    /// The file was unreadable, unparseable or of an unknown version. It has
    /// been moved aside and the daemon must start fresh (§8).
    Corrupt {
        /// Where the bad file now lives (`workspace.json.bad`).
        moved_to: PathBuf,
        /// What was wrong with it, for the warning.
        reason: String,
    },
}

/// The path a corrupt `workspace.json` is renamed to.
#[must_use]
pub fn bad_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bad");
    path.with_file_name(name)
}

/// Reads `workspace.json`, moving a corrupt file aside (§8).
pub fn load(path: &Path) -> Loaded {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => return quarantine(path, format!("cannot read it: {e}")),
    };

    let file: WorkspaceFile = match serde_json::from_str(&text) {
        Ok(file) => file,
        Err(e) => return quarantine(path, format!("it is not valid workspace.json: {e}")),
    };

    if file.version != WORKSPACE_FILE_VERSION {
        return quarantine(
            path,
            format!(
                "version {} is not the supported version {WORKSPACE_FILE_VERSION}",
                file.version
            ),
        );
    }

    Loaded::File(Box::new(file))
}

fn quarantine(path: &Path, reason: String) -> Loaded {
    let moved_to = bad_path(path);
    if let Err(e) = std::fs::rename(path, &moved_to) {
        tracing::warn!(path = %path.display(), error = %e, "cannot move the corrupt workspace file aside");
    }
    Loaded::Corrupt { moved_to, reason }
}

/// Writes `file` to `path` atomically: temp file, `fsync`, `rename` (§8).
pub fn write_atomic(path: &Path, file: &WorkspaceFile) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = file
        .to_bytes()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    {
        let mut handle = std::fs::File::create(&tmp)?;
        handle.write_all(&bytes)?;
        handle.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ------------------------------------------------------------ the debouncer

enum PersistMsg {
    Save(Box<WorkspaceFile>),
    Flush(oneshot::Sender<()>),
}

/// Handle onto the debouncing writer task (§8).
///
/// [`Persister::save`] is cheap and may be called on every mutation; the task
/// coalesces bursts into one write 500 ms after the last one.
/// [`Persister::flush`] bypasses the debounce, which is what `SIGTERM` and
/// `server.shutdown` do.
#[derive(Debug, Clone)]
pub struct Persister {
    tx: Option<mpsc::UnboundedSender<PersistMsg>>,
    path: PathBuf,
}

impl Persister {
    /// Spawns the writer task for `path`.
    #[must_use]
    pub fn spawn(path: PathBuf, metrics: Arc<Metrics>) -> Self {
        Self::spawn_with_debounce(path, metrics, DEBOUNCE)
    }

    /// [`Persister::spawn`] with a custom debounce, for tests.
    #[must_use]
    pub fn spawn_with_debounce(path: PathBuf, metrics: Arc<Metrics>, debounce: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run(rx, path.clone(), metrics, debounce));
        Self { tx: Some(tx), path }
    }

    /// A persister that writes nothing, for tests that do not care.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            tx: None,
            path: PathBuf::new(),
        }
    }

    /// The file being written.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Queues a write, restarting the debounce window.
    pub fn save(&self, file: WorkspaceFile) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(PersistMsg::Save(Box::new(file)));
        }
    }

    /// Writes any pending document now and waits for it to hit the disk.
    pub async fn flush(&self) {
        let Some(tx) = &self.tx else { return };
        let (done, wait) = oneshot::channel();
        if tx.send(PersistMsg::Flush(done)).is_ok() {
            let _ = wait.await;
        }
    }
}

async fn run(
    mut rx: mpsc::UnboundedReceiver<PersistMsg>,
    path: PathBuf,
    metrics: Arc<Metrics>,
    debounce: Duration,
) {
    let mut pending: Option<Box<WorkspaceFile>> = None;
    let deadline = tokio::time::sleep(Duration::from_secs(3600));
    tokio::pin!(deadline);
    let mut armed = false;

    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(PersistMsg::Save(file)) => {
                    pending = Some(file);
                    deadline.as_mut().reset(tokio::time::Instant::now() + debounce);
                    armed = true;
                }
                Some(PersistMsg::Flush(done)) => {
                    if let Some(file) = pending.take() {
                        write_now(&path, &file, &metrics);
                    }
                    armed = false;
                    let _ = done.send(());
                }
                None => {
                    if let Some(file) = pending.take() {
                        write_now(&path, &file, &metrics);
                    }
                    return;
                }
            },
            () = &mut deadline, if armed => {
                armed = false;
                if let Some(file) = pending.take() {
                    write_now(&path, &file, &metrics);
                }
            }
        }
    }
}

fn write_now(path: &Path, file: &WorkspaceFile, metrics: &Metrics) {
    match write_atomic(path, file) {
        Ok(()) => {
            metrics.persist_writes.inc();
            tracing::debug!(path = %path.display(), "wrote workspace.json");
        }
        Err(e) => {
            metrics.persist_errors.inc();
            tracing::warn!(path = %path.display(), error = %e, "cannot write workspace.json");
        }
    }
}

// ------------------------------------------------------------ time

/// Formats a [`SystemTime`] as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Hand-rolled rather than pulling in `chrono`: the daemon's dependency budget
/// (`HANDOVER.md` §6.9) does not stretch to a date library for one timestamp
/// that nothing parses.
#[must_use]
pub fn rfc3339(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 → (y, m, d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_proto::control::ViewState;

    use crate::workspace::model::SurfaceStatus;

    fn workspace() -> Workspace {
        let mut ws = Workspace::new();
        ws.insert_surface(Surface {
            id: SurfaceId(7),
            title: "zsh".into(),
            user_title: Some("build".into()),
            cwd: Some("/home/sonny/projects/x".into()),
            shell: vec!["/bin/zsh".into(), "-l".into()],
            cols: 200,
            rows: 60,
            has_foreground_child: false,
            status: SurfaceStatus::Running { pid: Some(4) },
            view: ViewState {
                scroll_offset: 12,
                selection: None,
            },
            pristine: false,
        });
        ws.seed_default_session(SurfaceId(7));
        ws
    }

    #[test]
    fn the_file_carries_the_shape_and_not_the_state() {
        let file = snapshot_file(&workspace());
        assert_eq!(file.version, WORKSPACE_FILE_VERSION);
        assert_eq!(file.sessions.len(), 1);
        let tab = &file.sessions[0].tabs[0];
        assert_eq!(tab.surface.cwd.as_deref(), Some("/home/sonny/projects/x"));
        assert_eq!(tab.surface.shell, vec!["/bin/zsh", "-l"]);
        assert_eq!(tab.surface.user_title.as_deref(), Some("build"));

        let json = serde_json::to_value(&file).unwrap();
        let text = json.to_string();
        assert!(
            !text.contains("scroll_offset"),
            "ViewState is not persisted"
        );
        assert!(!text.contains("pid"), "pids are not persisted");
        assert!(!text.contains("cols"), "the grid size is not persisted");
    }

    #[test]
    fn the_file_round_trips() {
        let file = snapshot_file(&workspace());
        let bytes = file.to_bytes().unwrap();
        assert!(bytes.ends_with(b"\n"));
        let back: WorkspaceFile = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, file);
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            load(&dir.path().join("workspace.json")),
            Loaded::Missing
        ));
    }

    #[test]
    fn a_corrupt_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let Loaded::Corrupt { moved_to, .. } = load(&path) else {
            panic!("expected corruption");
        };
        assert_eq!(moved_to, path.with_file_name("workspace.json.bad"));
        assert!(moved_to.exists());
        assert!(!path.exists());
    }

    #[test]
    fn an_unknown_version_is_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        std::fs::write(
            &path,
            br#"{"version":99,"saved_at":"x","next_id":1,"active_session":1,"sessions":[]}"#,
        )
        .unwrap();
        assert!(matches!(load(&path), Loaded::Corrupt { .. }));
    }

    #[test]
    fn writes_are_atomic_and_leave_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        write_atomic(&path, &snapshot_file(&workspace())).unwrap();
        assert!(path.exists());
        assert!(!path.with_extension("json.tmp").exists());
        assert!(matches!(load(&path), Loaded::File(_)));
    }

    #[test]
    fn timestamps_are_rfc3339() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(1_772_280_000)),
            "2026-02-28T12:00:00Z"
        );
    }
}
