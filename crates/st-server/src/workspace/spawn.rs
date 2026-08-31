//! The Surface-spawning seam between the control plane and the data plane.
//!
//! The Workspace actor never touches a PTY. It resolves a request into a
//! [`SpawnSpec`] — argv, cwd and a filtered environment — and hands it to a
//! [`SurfaceSpawner`], which allocates the [`SurfaceId`], starts the process
//! and (in the real implementation) registers the Surface with the data plane.
//!
//! **Ownership.** `crates/st-core` provides the production implementation,
//! wired up by `src/supervisor.rs` (owned by the data-plane agent): it opens a
//! `portable-pty` pair, spawns the shell with `set_controlling_tty(true)`,
//! starts the reader/writer/waiter threads described in `03-server.md` §4 and
//! reports title/cwd/exit back through
//! [`WorkspaceHandle`](crate::workspace::WorkspaceHandle). Everything in this
//! crate is written against the trait, so the control plane is testable with
//! [`NullSpawner`] and no processes at all.
//!
//! **Contract.**
//!
//! * `spawn` is called from the Workspace actor task and must not block for
//!   long: opening a PTY and `fork`/`exec` is fine, waiting for the shell to
//!   print a prompt is not.
//! * Surface ids must be unique for the life of the process and never reused
//!   (`02-protocol.md` §1 conventions), including across a `workspace.json`
//!   re-seed, which allocates brand-new ids for the restored shape.
//! * `kill` must be idempotent: the actor may signal a Surface whose child has
//!   already exited.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use st_proto::control::{ErrorBody, ErrorCode, KillSignal};
use st_proto::SurfaceId;

/// Everything the spawner needs to start one Surface.
///
/// This is the *resolved* form of the protocol's
/// [`st_proto::SpawnSpec`]: defaults from `config.toml` have been applied, the
/// cwd is absolute and the environment has already been filtered through the
/// grilling-Q48 allow-list, so the spawner performs no policy of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    /// argv; never empty, `argv[0]` is the program.
    pub shell: Vec<String>,
    /// Working directory; absolute and known to exist when the spec was built.
    pub cwd: PathBuf,
    /// Variables to set over the daemon's own environment, already filtered.
    pub env: BTreeMap<String, String>,
    /// Initial grid width.
    pub cols: u16,
    /// Initial grid height.
    pub rows: u16,
    /// `true` when this Surface is being auto-seeded (fresh start or a
    /// `workspace.json` re-seed) rather than asked for by a client. Such a
    /// Surface starts *pristine* and so counts as zero for idle exit
    /// (grilling Q42).
    pub seeded: bool,
}

impl SpawnSpec {
    /// The program name, used as the Surface's initial title.
    #[must_use]
    pub fn program_name(&self) -> String {
        self.shell
            .first()
            .map(|p| {
                PathBuf::from(p)
                    .file_name()
                    .map_or_else(|| p.clone(), |n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "shell".to_string())
    }
}

/// What a spawner reports back about a Surface it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnedSurface {
    /// The id it allocated. Unique for the life of the daemon.
    pub id: SurfaceId,
    /// The child's pid, when the implementation knows it.
    pub pid: Option<u32>,
    /// Initial title; `None` means "use the program name".
    pub title: Option<String>,
}

/// Why a spawn or a kill failed.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    /// The PTY or the shell could not start; maps to
    /// [`ErrorCode::SpawnFailed`].
    #[error("cannot spawn {program}: {source}")]
    Spawn {
        /// The program that could not be started.
        program: String,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// The spawner does not know this Surface; maps to [`ErrorCode::NotFound`].
    #[error("surface {0} is not known to the surface supervisor")]
    UnknownSurface(SurfaceId),
    /// Anything else; maps to [`ErrorCode::Internal`].
    #[error("{0}")]
    Other(String),
}

impl SpawnError {
    /// The control-plane error envelope for this failure.
    #[must_use]
    pub fn to_error_body(&self) -> ErrorBody {
        let code = match self {
            SpawnError::Spawn { .. } => ErrorCode::SpawnFailed,
            SpawnError::UnknownSurface(_) => ErrorCode::NotFound,
            SpawnError::Other(_) => ErrorCode::Internal,
        };
        ErrorBody::new(code, self.to_string())
    }
}

/// Starts and signals Surface processes.
///
/// Implemented by `NullSpawner` in tests and by the `st-core`-backed
/// supervisor in production (see the module docs).
pub trait SurfaceSpawner: Send + Sync + 'static {
    /// Starts one Surface and allocates its id.
    fn spawn(&self, spec: &SpawnSpec) -> Result<SpawnedSurface, SpawnError>;

    /// Sends `signal` to a Surface's process group (grilling Q21). Idempotent:
    /// signalling an already-dead Surface is not an error.
    fn kill(&self, id: SurfaceId, signal: KillSignal) -> Result<(), SpawnError>;

    /// Forgets a Surface permanently, releasing its engine, PTY and any
    /// remaining data-plane subscriptions.
    ///
    /// Called after [`kill`](Self::kill) when a Tab is closed (grilling Q21:
    /// closing a Tab destroys its Surface). Without it a killed Surface's
    /// entry leaks in the supervisor's registry for the daemon's lifetime.
    /// Idempotent; unknown ids are a no-op. The default implementation does
    /// nothing, which is correct for spawners that hold no state.
    fn destroy(&self, id: SurfaceId) {
        let _ = id;
    }
}

/// A [`SurfaceSpawner`] that starts no processes and fabricates ids.
///
/// Used by every control-plane test and as the daemon's placeholder until the
/// data plane is wired in: the Workspace behaves exactly as it would with real
/// shells, there is simply nothing on the other end of the Surface.
#[derive(Debug, Default)]
pub struct NullSpawner {
    next: AtomicU32,
    fail_next: std::sync::atomic::AtomicBool,
}

impl NullSpawner {
    /// A spawner whose first Surface id is 1.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next: AtomicU32::new(1),
            fail_next: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Makes the next [`SurfaceSpawner::spawn`] fail, so tests can exercise
    /// the `spawn_failed` path.
    pub fn fail_next_spawn(&self) {
        self.fail_next.store(true, Ordering::Relaxed);
    }
}

impl SurfaceSpawner for NullSpawner {
    fn spawn(&self, spec: &SpawnSpec) -> Result<SpawnedSurface, SpawnError> {
        if self.fail_next.swap(false, Ordering::Relaxed) {
            return Err(SpawnError::Spawn {
                program: spec.program_name(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
            });
        }
        Ok(SpawnedSurface {
            id: SurfaceId(self.next.fetch_add(1, Ordering::Relaxed)),
            pid: None,
            title: None,
        })
    }

    fn kill(&self, _id: SurfaceId, _signal: KillSignal) -> Result<(), SpawnError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SpawnSpec {
        SpawnSpec {
            shell: vec!["/bin/zsh".into(), "-l".into()],
            cwd: PathBuf::from("/tmp"),
            env: BTreeMap::new(),
            cols: 80,
            rows: 24,
            seeded: false,
        }
    }

    #[test]
    fn null_spawner_allocates_increasing_ids() {
        let spawner = NullSpawner::new();
        assert_eq!(spawner.spawn(&spec()).unwrap().id, SurfaceId(1));
        assert_eq!(spawner.spawn(&spec()).unwrap().id, SurfaceId(2));
        spawner.kill(SurfaceId(1), KillSignal::Term).unwrap();
        spawner.kill(SurfaceId(99), KillSignal::Kill).unwrap();
    }

    #[test]
    fn a_failed_spawn_maps_to_spawn_failed() {
        let spawner = NullSpawner::new();
        spawner.fail_next_spawn();
        let err = spawner.spawn(&spec()).unwrap_err();
        assert_eq!(err.to_error_body().code, ErrorCode::SpawnFailed);
        assert!(spawner.spawn(&spec()).is_ok(), "only the next spawn fails");
    }

    #[test]
    fn program_name_is_the_file_name() {
        assert_eq!(spec().program_name(), "zsh");
    }
}
