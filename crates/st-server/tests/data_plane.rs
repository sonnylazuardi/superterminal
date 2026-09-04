//! End-to-end DATA plane tests over a real Unix socket.
//!
//! Every test binds a temporary socket, runs the daemon's real accept path
//! ([`st_server::data::accept_with_magic`]) against the real
//! [`SurfaceSupervisor`], and talks to it with an `st-proto` codec — no mocks
//! between the client bytes and the terminal engine.
//!
//! Tests that need a child process spawn one through the real
//! [`SurfaceSpawner`] and skip themselves when `/bin/sh` is missing. Everything
//! else uses a PTY-less Surface fed directly, which makes the coalescing,
//! paging and View State rules deterministic.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use st_core::surface::{Surface, SurfaceConfig};
use st_core::vt::alacritty::EngineConfig;
use st_proto::control::{AbsPoint, KillSignal, Selection, SelectionKind};
use st_proto::data::SetViewState;
use st_proto::{
    AbsLine, Ack, Attach, AttachMode, CellFlags, ClientKind, DataMsg, Detach, FetchHistory,
    FrameDecoder, Hello, Input, ProtoVersion, RejectReason, Resize, Row, Seq, SurfaceId,
    DATA_ERR_SURFACE_EXITED, DATA_MAGIC, PROTO_VERSION,
};
use st_server::data::{accept_with_magic, DataCtx};
use st_server::supervisor::{
    RecordingNotifier, SupervisorConfig, SurfaceSlot, SurfaceSupervisor, Upcall,
};
use st_server::workspace::spawn::{SpawnSpec, SurfaceSpawner};
use st_server::workspace::ClientId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

/// Long enough that a slow CI box never flakes, short enough that a real
/// failure does not hang the suite.
const DEADLINE: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------- harness

struct Harness {
    supervisor: Arc<SurfaceSupervisor>,
    notifier: Arc<RecordingNotifier>,
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sock");
        let notifier = Arc::new(RecordingNotifier::new());
        let supervisor = Arc::new(SurfaceSupervisor::new(
            SupervisorConfig {
                scrollback_lines: 200,
                build_id: "test".to_owned(),
                // Sample often so cwd/title/foreground reporting is testable.
                sample_interval: Duration::from_millis(100),
                ..SupervisorConfig::default()
            },
            notifier.clone(),
        ));

        let listener = UnixListener::bind(&path).expect("bind");
        let accept_supervisor = Arc::clone(&supervisor);
        tokio::spawn(async move {
            let mut next = 0u64;
            while let Ok((stream, _)) = listener.accept().await {
                next += 1;
                let ctx = DataCtx::new(Arc::clone(&accept_supervisor))
                    .with_build_id("test")
                    .with_workspace_revision(7);
                tokio::spawn(accept_with_magic(stream.into(), ctx, ClientId(next)));
            }
        });

        Self {
            supervisor,
            notifier,
            path,
            _dir: dir,
        }
    }

    async fn connect(&self) -> Client {
        Client::connect(&self.path).await
    }

    /// A Surface with no PTY, fed by the test instead of by a child process.
    fn engine_surface(&self, id: SurfaceId, cols: u16, rows: u16) -> Arc<SurfaceSlot> {
        let surface = Surface::new(SurfaceConfig {
            id,
            engine: EngineConfig {
                cols,
                rows,
                scrollback_lines: 200,
                default_title: "test".to_owned(),
                kitty_keyboard: true,
            },
            pty: None,
            spawn_cwd: PathBuf::from("/"),
            ..SurfaceConfig::default()
        })
        .expect("a PTY-less Surface never fails");
        self.supervisor
            .insert_surface(surface)
            .expect("insert_surface")
    }

    /// Spawns a real `/bin/sh -c <script>` Surface, or `None` when there is no
    /// shell to spawn (the test then skips itself).
    fn shell_surface(&self, script: &str) -> Option<SurfaceId> {
        if !Path::new("/bin/sh").exists() {
            return None;
        }
        let spec = SpawnSpec {
            shell: vec!["/bin/sh".to_owned(), "-c".to_owned(), script.to_owned()],
            cwd: std::env::temp_dir(),
            env: BTreeMap::new(),
            cols: 80,
            rows: 10,
            seeded: false,
        };
        match self.supervisor.spawn(&spec) {
            Ok(spawned) => Some(spawned.id),
            Err(err) => {
                eprintln!("skipping: cannot spawn a shell: {err}");
                None
            }
        }
    }
}

struct Client {
    stream: UnixStream,
    decoder: FrameDecoder,
    buf: Vec<u8>,
}

impl Client {
    async fn connect(path: &Path) -> Self {
        let mut stream = UnixStream::connect(path).await.expect("connect");
        stream.write_all(&DATA_MAGIC).await.expect("magic");
        Self {
            stream,
            decoder: FrameDecoder::new(),
            buf: vec![0u8; 64 * 1024],
        }
    }

    async fn send(&mut self, msg: &DataMsg) {
        let mut wire = Vec::new();
        msg.encode_to(&mut wire).expect("encode");
        self.stream.write_all(&wire).await.expect("write");
    }

    async fn hello(&mut self, version: ProtoVersion) -> DataMsg {
        self.send(&DataMsg::Hello(Hello {
            proto_version: version,
            client_kind: ClientKind::Data,
            build_id: "data_plane test".to_owned(),
        }))
        .await;
        self.recv().await.expect("a handshake reply")
    }

    async fn handshake(&mut self) {
        match self.hello(PROTO_VERSION).await {
            DataMsg::HelloAck(ack) => assert_eq!(ack.proto_version, PROTO_VERSION),
            other => panic!("expected HelloAck, got {other:?}"),
        }
    }

    /// Reads one message, or `None` if nothing arrives within `timeout`.
    async fn recv_within(&mut self, timeout: Duration) -> Option<DataMsg> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(frame) = self.decoder.next_frame().expect("framing") {
                return Some(DataMsg::from_frame(frame.msg_type, &frame.payload).expect("decode"));
            }
            let read = tokio::time::timeout_at(deadline, self.stream.read(&mut self.buf))
                .await
                .ok()?
                .expect("read");
            if read == 0 {
                return None;
            }
            let chunk = self.buf[..read].to_vec();
            self.decoder.push(&chunk);
        }
    }

    async fn recv(&mut self) -> Option<DataMsg> {
        self.recv_within(DEADLINE).await
    }

    /// Reads until `pick` returns `Some`, or the deadline passes.
    async fn wait_for<T>(&mut self, mut pick: impl FnMut(&DataMsg) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!left.is_zero(), "timed out waiting for a message");
            let msg = self
                .recv_within(left)
                .await
                .expect("the connection closed while waiting");
            if let Some(value) = pick(&msg) {
                return value;
            }
        }
    }

    async fn attach(&mut self, surface: SurfaceId, mode: AttachMode) {
        self.send(&DataMsg::Attach(Attach {
            surface_id: surface,
            mode,
            want_snapshot: true,
            known_seq: Seq::ZERO,
        }))
        .await;
    }
}

// ------------------------------------------------------------------ helpers

fn render_row(row: &Row) -> String {
    let mut out = String::new();
    for cell in &row.cells {
        if cell.flags.contains(CellFlags::GRAPHEME_EXT) {
            if let Some(text) = row.extras.get(cell.codepoint as usize) {
                out.push_str(text);
            }
        } else if cell.codepoint != 0 {
            if let Some(ch) = char::from_u32(cell.codepoint) {
                out.push(ch);
            }
        }
    }
    out
}

fn render_grid(rows: &[Row]) -> String {
    rows.iter().map(render_row).collect::<Vec<_>>().join("\n")
}

/// Polls `cond` until it holds, or panics after [`DEADLINE`].
async fn wait_until(mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while !cond() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition never became true"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn selection() -> Selection {
    Selection {
        kind: SelectionKind::Normal,
        anchor: AbsPoint {
            line: AbsLine::new(1),
            col: 2,
        },
        head: AbsPoint {
            line: AbsLine::new(3),
            col: 4,
        },
    }
}

// -------------------------------------------------------------------- tests

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_handshake_answers_hello_with_hello_ack() {
    let harness = Harness::start();
    let mut client = harness.connect().await;

    match client.hello(PROTO_VERSION).await {
        DataMsg::HelloAck(ack) => {
            assert_eq!(ack.proto_version, PROTO_VERSION);
            assert_eq!(ack.server_build_id, "test");
            assert_eq!(ack.workspace_revision, 7);
            assert_eq!(ack.server_pid, std::process::id());
        }
        other => panic!("expected HelloAck, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_major_version_is_rejected_and_the_connection_closes() {
    let harness = Harness::start();
    let mut client = harness.connect().await;

    match client
        .hello(ProtoVersion::new(PROTO_VERSION.major + 1, 0))
        .await
    {
        DataMsg::Reject(reject) => {
            assert_eq!(reject.reason, RejectReason::MajorMismatch);
            assert_eq!(reject.server_version, PROTO_VERSION);
            assert!(reject.message.contains("major"), "{}", reject.message);
        }
        other => panic!("expected Reject, got {other:?}"),
    }
    assert!(
        client.recv_within(Duration::from_secs(2)).await.is_none(),
        "the server must close after a Reject"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_active_attach_snapshots_the_shell_output() {
    let harness = Harness::start();
    let Some(surface) = harness.shell_surface("printf 'hello\\n'; sleep 60") else {
        return;
    };
    let slot = harness.supervisor.slot(surface).expect("slot");

    // Wait for the engine to have consumed the child's output, so the very
    // first frame after Attach must already contain it.
    wait_until(|| slot.lock().has_pending()).await;

    let mut client = harness.connect().await;
    client.handshake().await;
    client.attach(surface, AttachMode::Active).await;

    let grid = client
        .wait_for(|msg| match msg {
            DataMsg::Snapshot(snapshot) => Some(render_grid(&snapshot.grid)),
            _ => None,
        })
        .await;
    assert!(grid.contains("hello"), "snapshot grid was:\n{grid}");

    let _ = harness.supervisor.kill(surface, KillSignal::Kill);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delta_carries_the_dirty_rows_and_chains_on_since_seq() {
    let harness = Harness::start();
    let surface = SurfaceId(1);
    let slot = harness.engine_surface(surface, 40, 6);
    // Alacritty reports full damage for the first `advance`; with nobody
    // attached it is discarded, so the Delta below is genuinely incremental.
    slot.lock().feed(b"first line\r\n");

    let mut client = harness.connect().await;
    client.handshake().await;
    client.attach(surface, AttachMode::Active).await;

    let first_seq = client
        .wait_for(|msg| match msg {
            DataMsg::Snapshot(snapshot) => Some(snapshot.seq),
            _ => None,
        })
        .await;
    client
        .send(&DataMsg::Ack(Ack {
            surface_id: surface,
            seq: first_seq,
        }))
        .await;

    slot.lock().feed(b"\r\nsecond line\r\n");

    let delta = client
        .wait_for(|msg| match msg {
            DataMsg::Delta(delta) => Some(delta.clone()),
            _ => None,
        })
        .await;

    assert_eq!(
        delta.since_seq, first_seq,
        "the Delta chains onto the Snapshot"
    );
    assert_eq!(delta.seq.get(), first_seq.get() + 1);
    assert!(!delta.rows.is_empty(), "a Delta must carry its dirty rows");
    let text: String = delta.rows.iter().map(|r| render_row(&r.row)).collect();
    assert!(text.contains("second line"), "delta rows were:\n{text}");
    // Row-granular damage (Q16): only the rows that changed.
    assert!(
        delta.rows.len() < 6,
        "an unchanged grid must not be resent whole: {:?}",
        delta.rows.iter().map(|r| r.index).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_passive_attach_gets_title_and_exit_but_never_rows() {
    let harness = Harness::start();
    let surface = SurfaceId(1);
    let slot = harness.engine_surface(surface, 40, 6);

    let mut client = harness.connect().await;
    client.handshake().await;
    client.attach(surface, AttachMode::Passive).await;

    let snapshot = client
        .wait_for(|msg| match msg {
            DataMsg::Snapshot(snapshot) => Some(snapshot.clone()),
            _ => None,
        })
        .await;
    assert!(
        snapshot.grid.is_empty(),
        "Q44: a Passive attach never receives rows"
    );
    assert!(snapshot.styles.is_empty());
    assert_eq!(snapshot.rows, 6, "the geometry is still reported");

    client
        .send(&DataMsg::Ack(Ack {
            surface_id: surface,
            seq: snapshot.seq,
        }))
        .await;

    // A title change and a screenful of output: only the title may come back.
    slot.lock()
        .feed(b"\x1b]0;passive title\x07lots of text here\r\n");

    let delta = client
        .wait_for(|msg| match msg {
            DataMsg::Delta(delta) => Some(delta.clone()),
            _ => None,
        })
        .await;
    assert!(delta.rows.is_empty(), "Q44: still no rows");
    assert_eq!(delta.title.as_deref(), Some("passive title"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_passive_attach_is_told_when_the_child_exits() {
    let harness = Harness::start();
    let Some(surface) = harness.shell_surface("sleep 1; exit 3") else {
        return;
    };

    let mut client = harness.connect().await;
    client.handshake().await;
    client.attach(surface, AttachMode::Passive).await;

    let status = client
        .wait_for(|msg| match msg {
            DataMsg::SurfaceExited(exited) if exited.surface_id == surface => Some(exited.status),
            DataMsg::Snapshot(snapshot) => snapshot.exited,
            _ => None,
        })
        .await;
    assert_eq!(status.code, Some(3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_reaches_the_pty_and_comes_back_as_output() {
    let harness = Harness::start();
    let Some(surface) = harness.shell_surface("cat") else {
        return;
    };

    let mut client = harness.connect().await;
    client.handshake().await;
    client.attach(surface, AttachMode::Active).await;
    let seq = client
        .wait_for(|msg| match msg {
            DataMsg::Snapshot(snapshot) => Some(snapshot.seq),
            _ => None,
        })
        .await;
    client
        .send(&DataMsg::Ack(Ack {
            surface_id: surface,
            seq,
        }))
        .await;

    client
        .send(&DataMsg::Input(Input {
            surface_id: surface,
            bytes: b"echo-me\n".to_vec(),
        }))
        .await;

    let mut seen = String::new();
    client
        .wait_for(|msg| {
            match msg {
                DataMsg::Delta(delta) => {
                    for row in &delta.rows {
                        seen.push_str(&render_row(&row.row));
                    }
                }
                DataMsg::Snapshot(snapshot) => seen = render_grid(&snapshot.grid),
                _ => {}
            }
            seen.contains("echo-me").then_some(())
        })
        .await;

    // Typing makes the Surface non-pristine (Q42).
    assert!(
        harness.notifier.upcalls().iter().any(|u| matches!(
            u,
            Upcall::Surface(st_server::workspace::SurfaceEvent::Input { .. })
        )),
        "Input must be reported to the Workspace actor"
    );

    let _ = harness.supervisor.kill(surface, KillSignal::Kill);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resize_changes_the_grid_and_clears_the_selection() {
    let harness = Harness::start();
    let surface = SurfaceId(1);
    let slot = harness.engine_surface(surface, 40, 6);

    let mut client = harness.connect().await;
    client.handshake().await;
    client.attach(surface, AttachMode::Active).await;
    let seq = client
        .wait_for(|msg| match msg {
            DataMsg::Snapshot(snapshot) => Some(snapshot.seq),
            _ => None,
        })
        .await;
    client
        .send(&DataMsg::Ack(Ack {
            surface_id: surface,
            seq,
        }))
        .await;

    client
        .send(&DataMsg::SetViewState(SetViewState {
            surface,
            scroll_offset: None,
            selection: Some(selection()),
        }))
        .await;
    wait_until(|| slot.lock().view_state().selection.is_some()).await;

    client
        .send(&DataMsg::Resize(Resize {
            surface_id: surface,
            cols: 100,
            rows: 20,
        }))
        .await;

    let resized = client
        .wait_for(|msg| match msg {
            DataMsg::Delta(delta) => delta.resized,
            DataMsg::Snapshot(snapshot) => Some((snapshot.cols, snapshot.rows)),
            _ => None,
        })
        .await;
    assert_eq!(resized, (100, 20));
    assert_eq!(slot.lock().size(), (100, 20));
    assert!(
        slot.lock().view_state().selection.is_none(),
        "Q40: a resize clears the selection"
    );
    assert!(
        harness.notifier.upcalls().iter().any(|u| matches!(
            u,
            Upcall::ViewState { view, .. } if view.selection.is_none()
        )),
        "the cleared View State must be broadcast"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_history_pages_with_stable_absolute_line_ids() {
    let harness = Harness::start();
    let surface = SurfaceId(1);
    let slot = harness.engine_surface(surface, 40, 4);

    for line in 0..20u32 {
        slot.lock().feed(format!("line {line}\r\n").as_bytes());
    }
    let (base, len) = {
        let surface = slot.lock();
        (surface.history_base(), surface.history_len())
    };
    assert!(len >= 10, "expected some scrollback, got {len}");

    let mut client = harness.connect().await;
    client.handshake().await;

    let fetch = |from: u64, count: u16| {
        DataMsg::FetchHistory(FetchHistory {
            surface_id: surface,
            from_line: AbsLine::new(from),
            count,
        })
    };

    client.send(&fetch(base.get(), 10)).await;
    let whole = client
        .wait_for(|msg| match msg {
            DataMsg::History(page) => Some(page.clone()),
            _ => None,
        })
        .await;
    assert_eq!(whole.from_line, base);
    assert_eq!(whole.rows.len(), 10);

    client.send(&fetch(base.get(), 5)).await;
    let first = client
        .wait_for(|msg| match msg {
            DataMsg::History(page) => Some(page.clone()),
            _ => None,
        })
        .await;
    client.send(&fetch(base.get() + 5, 5)).await;
    let second = client
        .wait_for(|msg| match msg {
            DataMsg::History(page) => Some(page.clone()),
            _ => None,
        })
        .await;

    assert_eq!(first.from_line, base);
    assert_eq!(second.from_line, AbsLine::new(base.get() + 5));
    assert_eq!(first.history_base, base);

    let paged: Vec<String> = first
        .rows
        .iter()
        .chain(second.rows.iter())
        .map(render_row)
        .collect();
    let whole: Vec<String> = whole.rows.iter().map(render_row).collect();
    assert_eq!(
        paged, whole,
        "the same absolute ids must return the same lines whatever the paging"
    );
    assert!(
        paged[0].starts_with("line 0"),
        "first history line: {paged:?}"
    );

    // A request below `history_base` is clamped, not an error (§8).
    client.send(&fetch(0, 3)).await;
    let clamped = client
        .wait_for(|msg| match msg {
            DataMsg::History(page) => Some(page.clone()),
            _ => None,
        })
        .await;
    assert!(clamped.from_line >= base);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_view_state_is_stored_on_the_surface_and_reported_upward() {
    let harness = Harness::start();
    let surface = SurfaceId(1);
    let slot = harness.engine_surface(surface, 40, 6);
    for line in 0..12u32 {
        slot.lock().feed(format!("line {line}\r\n").as_bytes());
    }
    let first_visible = {
        let surface = slot.lock();
        surface.history_base().get() + surface.history_len()
    };

    let mut client = harness.connect().await;
    client.handshake().await;
    client
        .send(&DataMsg::SetViewState(SetViewState {
            surface,
            // Three lines above the bottom of the scroll region.
            scroll_offset: Some(AbsLine::new(first_visible - 3)),
            selection: Some(selection()),
        }))
        .await;

    wait_until(|| slot.lock().view_state().selection.is_some()).await;
    let stored = slot.lock().view_state().clone();
    assert_eq!(stored.scroll_offset, 3);
    assert_eq!(stored.selection, Some(selection()));

    let echoed = harness.notifier.upcalls().into_iter().any(|upcall| {
        matches!(upcall, Upcall::ViewState { surface: s, view, .. }
            if s == surface && view.scroll_offset == 3 && view.selection == Some(selection()))
    });
    assert!(echoed, "Q43/Q49: the edit must reach the Workspace actor");

    // A detach for a Surface we never attached is a per-message error, and the
    // connection survives it.
    client
        .send(&DataMsg::Detach(Detach {
            surface_id: surface,
        }))
        .await;
    let code = client
        .wait_for(|msg| match msg {
            DataMsg::DataError(err) => Some(err.code),
            _ => None,
        })
        .await;
    assert_eq!(code, st_proto::DATA_ERR_NOT_ATTACHED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_to_an_exited_surface_is_a_per_message_error() {
    let harness = Harness::start();
    let Some(surface) = harness.shell_surface("exit 7") else {
        return;
    };
    let slot = harness.supervisor.slot(surface).expect("slot");

    let mut client = harness.connect().await;
    client.handshake().await;
    client.attach(surface, AttachMode::Active).await;

    client
        .wait_for(|msg| match msg {
            DataMsg::SurfaceExited(exited) => Some(exited.status),
            DataMsg::Snapshot(snapshot) => snapshot.exited,
            _ => None,
        })
        .await;
    assert!(!slot.lock().status().is_running());

    client
        .send(&DataMsg::Input(Input {
            surface_id: surface,
            bytes: b"ls\r".to_vec(),
        }))
        .await;
    let err = client
        .wait_for(|msg| match msg {
            DataMsg::DataError(err) => Some(err.clone()),
            _ => None,
        })
        .await;
    assert_eq!(err.code, DATA_ERR_SURFACE_EXITED);
    assert_eq!(err.surface_id, Some(surface));

    // Q48: not connection-fatal — the next request is still answered.
    client
        .send(&DataMsg::FetchHistory(FetchHistory {
            surface_id: surface,
            from_line: AbsLine::ZERO,
            count: 4,
        }))
        .await;
    client
        .wait_for(|msg| matches!(msg, DataMsg::History(_)).then_some(()))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn withholding_acks_coalesces_instead_of_queueing_without_bound() {
    let harness = Harness::start();
    let surface = SurfaceId(1);
    let slot = harness.engine_surface(surface, 40, 6);

    let mut client = harness.connect().await;
    client.handshake().await;
    client.attach(surface, AttachMode::Active).await;

    // Never acknowledge anything, but keep the Surface busy for well over the
    // 8.33 ms flush interval.
    let mut state_frames = 0usize;
    let mut last_seq = Seq::ZERO;
    for round in 0..60u32 {
        slot.lock()
            .feed(format!("\r\nround {round} of noisy output").as_bytes());
        tokio::time::sleep(Duration::from_millis(20)).await;
        while let Some(msg) = client.recv_within(Duration::from_millis(1)).await {
            match msg {
                DataMsg::Snapshot(snapshot) => {
                    state_frames += 1;
                    last_seq = snapshot.seq;
                }
                DataMsg::Delta(delta) => {
                    state_frames += 1;
                    last_seq = delta.seq;
                }
                _ => {}
            }
        }
    }

    // One Snapshot plus at most `MAX_UNACKED_DELTAS` Deltas may be in flight,
    // and the 3 s slow-client rule adds one forced Snapshot per stall window.
    let cap = 1 + st_proto::MAX_UNACKED_DELTAS as usize + 2;
    assert!(
        state_frames <= cap,
        "the ack window must stop the server queueing: got {state_frames} frames, cap {cap}"
    );
    assert!(state_frames >= 1, "the first Snapshot must always be sent");

    // Acking reopens the window, and what comes back is the *latest* state,
    // not a replay of the 60 rounds (Q27).
    client
        .send(&DataMsg::Ack(Ack {
            surface_id: surface,
            seq: last_seq,
        }))
        .await;
    slot.lock().feed(b"\r\nfinal line");

    let mut seen = String::new();
    client
        .wait_for(|msg| {
            match msg {
                DataMsg::Delta(delta) => {
                    for row in &delta.rows {
                        seen.push_str(&render_row(&row.row));
                    }
                }
                DataMsg::Snapshot(snapshot) => seen = render_grid(&snapshot.grid),
                _ => {}
            }
            seen.contains("final line").then_some(())
        })
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detaching_and_closing_release_the_subscription() {
    let harness = Harness::start();
    let surface = SurfaceId(1);
    let slot = harness.engine_surface(surface, 40, 6);

    {
        let mut client = harness.connect().await;
        client.handshake().await;
        client.attach(surface, AttachMode::Active).await;
        client
            .wait_for(|msg| matches!(msg, DataMsg::Snapshot(_)).then_some(()))
            .await;
        assert_eq!(slot.lock().publisher().len(), 1);

        client
            .send(&DataMsg::Detach(Detach {
                surface_id: surface,
            }))
            .await;
        client
            .wait_for(|msg| match msg {
                DataMsg::Detached(detached) => Some(detached.reason),
                _ => None,
            })
            .await;
        assert_eq!(slot.lock().publisher().len(), 0);

        // Re-attach, then drop the connection without detaching.
        client.attach(surface, AttachMode::Active).await;
        client
            .wait_for(|msg| matches!(msg, DataMsg::Snapshot(_)).then_some(()))
            .await;
        assert_eq!(slot.lock().publisher().len(), 1);
    }

    wait_until(|| slot.lock().publisher().is_empty()).await;
    assert_eq!(harness.supervisor.client_count(), 0);
}
