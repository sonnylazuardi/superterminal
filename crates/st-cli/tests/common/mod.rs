//! A fake superterminald that speaks the real wire protocol.
//!
//! The harness binds a Unix socket in a temp directory, sniffs the first byte
//! exactly the way `02-protocol.md` §1.2 says a server must (`{` → CONTROL,
//! `0xFF` → DATA), and then runs a scripted conversation. Nothing is stubbed
//! at the codec level: `st` writes real NDJSON and real postcard frames and
//! the harness decodes them with `st_proto`, so a change to either codec
//! breaks these tests.

#![allow(dead_code)] // each integration test file uses a different subset

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

use serde_json::{json, Value};
use st_proto::{encode_frame, DataMsg, FrameDecoder, HelloAck, PROTO_VERSION};

/// What the fake server should do with a CONTROL request, keyed by its `t`.
pub type ControlHandler = Box<dyn Fn(&Value) -> Value + Send + 'static>;

/// What the fake server should send on the DATA plane after `Attach`.
pub type DataHandler = Box<dyn Fn(&DataMsg) -> Vec<DataMsg> + Send + 'static>;

/// A running fake server. Dropping it stops the accept loop and removes the
/// temp directory.
pub struct FakeServer {
    dir: tempfile::TempDir,
    socket: PathBuf,
    handle: Option<JoinHandle<()>>,
    seen: mpsc::Receiver<Seen>,
    stop: Arc<AtomicBool>,
}

/// What the fake server observed, so tests can assert on what `st` sent.
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    /// A CONTROL line arrived, parsed as JSON.
    Control(Value),
    /// A DATA message arrived.
    Data(String),
}

/// Builds a [`FakeServer`].
pub struct FakeServerBuilder {
    control: Option<ControlHandler>,
    data: Option<DataHandler>,
    reject_handshake: bool,
    close_immediately: bool,
    server_build_id: String,
    server_pid: u32,
}

impl Default for FakeServerBuilder {
    fn default() -> Self {
        Self {
            control: None,
            data: None,
            reject_handshake: false,
            close_immediately: false,
            server_build_id: "fake-build-cafe".into(),
            server_pid: 4242,
        }
    }
}

impl FakeServerBuilder {
    /// Answers CONTROL requests with `handler(request) -> result`. The harness
    /// wraps the returned value in `{"t":"ok","id":…,"result":…}`; return a
    /// value with a `"__err"` key to produce an `err` envelope instead.
    #[must_use]
    pub fn control(mut self, handler: impl Fn(&Value) -> Value + Send + 'static) -> Self {
        self.control = Some(Box::new(handler));
        self
    }

    /// Answers DATA messages with `handler(message) -> messages to send`.
    #[must_use]
    pub fn data(mut self, handler: impl Fn(&DataMsg) -> Vec<DataMsg> + Send + 'static) -> Self {
        self.data = Some(Box::new(handler));
        self
    }

    /// Reject the handshake instead of acknowledging it.
    #[must_use]
    pub fn rejecting(mut self) -> Self {
        self.reject_handshake = true;
        self
    }

    /// Accept the connection and immediately close it, saying nothing.
    #[must_use]
    pub fn silent(mut self) -> Self {
        self.close_immediately = true;
        self
    }

    /// The `server_build_id` reported in `hello.ack`.
    #[must_use]
    pub fn build_id(mut self, id: &str) -> Self {
        self.server_build_id = id.into();
        self
    }

    /// Binds the socket and starts serving.
    #[must_use]
    pub fn start(self) -> FakeServer {
        let dir = tempfile::tempdir().expect("temp dir");
        let socket = dir.path().join("server.sock");
        let listener = UnixListener::bind(&socket).expect("bind");
        let (tx, seen) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));

        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if thread_stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { break };
                // A client hanging up mid-conversation is normal.
                let _ = serve(&self, stream, &tx);
                if thread_stop.load(Ordering::SeqCst) {
                    break;
                }
            }
        });

        FakeServer {
            dir,
            socket,
            handle: Some(handle),
            seen,
            stop,
        }
    }
}

impl FakeServer {
    /// Starts building one.
    #[must_use]
    pub fn builder() -> FakeServerBuilder {
        FakeServerBuilder::default()
    }

    /// The socket path to hand to `st --socket`.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// The directory holding the socket, where a lockfile can be written.
    #[must_use]
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// Everything the server has observed so far, without waiting.
    #[must_use]
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.try_iter().collect()
    }

    /// Waits (up to 5 s) until at least `n` messages have been observed, then
    /// returns everything seen. The server runs on its own thread, so a test
    /// that asserts on what `st` sent must not race its last frame.
    #[must_use]
    pub fn wait_seen(&self, n: usize) -> Vec<Seen> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut out = Vec::new();
        while out.len() < n {
            let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) else {
                break;
            };
            match self.seen.recv_timeout(left) {
                Ok(seen) => out.push(seen),
                Err(_) => break,
            }
        }
        out.extend(self.seen.try_iter());
        out
    }

    /// Runs the `st` binary against this server with the given arguments.
    #[must_use]
    pub fn run(&self, args: &[&str]) -> Output {
        run_st(Some(&self.socket), args)
    }

    /// The same, but pointing `st` at the socket through
    /// `$SUPERTERMINAL_SOCKET` instead of `--socket`.
    #[must_use]
    pub fn run_via_env(&self, args: &[&str]) -> Output {
        run_st_env(
            None,
            &[("SUPERTERMINAL_SOCKET", self.socket.to_str().unwrap())],
            args,
        )
    }
}

/// Runs the `st` binary, optionally pointing it at a socket with `--socket`.
///
/// `$SUPERTERMINAL_SOCKET` and `$SUPERTERMINAL_LOG` are cleared so the tests
/// never reach the developer's real server or pick up their log settings.
#[must_use]
pub fn run_st(socket: Option<&Path>, args: &[&str]) -> Output {
    run_st_env(socket, &[], args)
}

/// [`run_st`] plus extra environment variables, for the precedence tests.
#[must_use]
pub fn run_st_env(socket: Option<&Path>, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_st"));
    cmd.env_remove("SUPERTERMINAL_SOCKET");
    cmd.env_remove("SUPERTERMINAL_LOG");
    for (key, value) in env {
        cmd.env(key, value);
    }
    if let Some(socket) = socket {
        cmd.arg("--socket").arg(socket);
    }
    cmd.args(args);
    cmd.output().expect("run st")
}

/// Stdout of a completed run, as a string.
#[must_use]
pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Stderr of a completed run, as a string.
#[must_use]
pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The exit status code, or `-1` if the process was signalled.
#[must_use]
pub fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn serve(
    cfg: &FakeServerBuilder,
    stream: UnixStream,
    tx: &mpsc::Sender<Seen>,
) -> std::io::Result<()> {
    // §1.2: classify by the first byte. `BufReader::fill_buf` looks at it
    // without consuming it, which is what a real server's sniffer does.
    let writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let first = match reader.fill_buf()? {
        [] => return Ok(()),
        buf => buf[0],
    };
    match first {
        b'{' => serve_control(cfg, reader, writer, tx),
        0xFF => serve_data(cfg, reader, writer, tx),
        _ => Ok(()),
    }
}

fn serve_control(
    cfg: &FakeServerBuilder,
    mut reader: BufReader<UnixStream>,
    mut writer: UnixStream,
    tx: &mpsc::Sender<Seen>,
) -> std::io::Result<()> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let hello: Value = serde_json::from_str(&line).expect("hello is JSON");
    let _ = tx.send(Seen::Control(hello.clone()));
    assert_eq!(hello["t"], "hello", "first control line must be a hello");

    if cfg.close_immediately {
        return Ok(());
    }
    if cfg.reject_handshake {
        writeln!(
            writer,
            "{}",
            json!({
                "t": "reject",
                "reason": "major_mismatch",
                "message": "server speaks 2.x",
                "server_version": "2.0",
            })
        )?;
        return Ok(());
    }

    writeln!(
        writer,
        "{}",
        json!({
            "t": "hello.ack",
            "proto_version": PROTO_VERSION.to_string(),
            "server_build_id": cfg.server_build_id,
            "workspace_revision": 42,
            "server_pid": cfg.server_pid,
        })
    )?;

    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            return Ok(());
        };
        let _ = tx.send(Seen::Control(req.clone()));
        let id = req["id"].clone();
        let Some(handler) = &cfg.control else {
            writeln!(
                writer,
                "{}",
                json!({
                    "t": "err",
                    "id": id,
                    "error": {"code": "unsupported", "message": "no handler"},
                })
            )?;
            continue;
        };
        let result = handler(&req);
        let response = if let Some(err) = result.get("__err") {
            json!({"t": "err", "id": id, "error": err})
        } else {
            json!({"t": "ok", "id": id, "result": result})
        };
        writeln!(writer, "{response}")?;
    }
}

fn serve_data(
    cfg: &FakeServerBuilder,
    mut reader: BufReader<UnixStream>,
    mut writer: UnixStream,
    tx: &mpsc::Sender<Seen>,
) -> std::io::Result<()> {
    let mut decoder = FrameDecoder::expecting_magic();
    let mut buf = [0u8; 8192];

    fn send(writer: &mut UnixStream, msg: &DataMsg) -> std::io::Result<()> {
        let payload = msg.to_payload().expect("encode");
        let mut wire = Vec::new();
        encode_frame(msg.msg_type(), &payload, &mut wire).expect("frame");
        writer.write_all(&wire)?;
        writer.flush()
    }

    let mut greeted = false;
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            return Ok(());
        }
        decoder.push(&buf[..read]);
        while let Some(frame) = decoder.next_frame().expect("framing") {
            let msg = DataMsg::from_frame(frame.msg_type, &frame.payload).expect("decode");
            let _ = tx.send(Seen::Data(format!("{msg:?}")));

            if !greeted {
                assert!(
                    matches!(msg, DataMsg::Hello(_)),
                    "the first data frame must be a Hello, got 0x{:04X}",
                    frame.msg_type
                );
                greeted = true;
                if cfg.close_immediately {
                    return Ok(());
                }
                if cfg.reject_handshake {
                    send(
                        &mut writer,
                        &DataMsg::Reject(st_proto::Reject {
                            reason: st_proto::RejectReason::MajorMismatch,
                            message: "server speaks 2.x".into(),
                            server_version: st_proto::ProtoVersion::new(2, 0),
                        }),
                    )?;
                    return Ok(());
                }
                send(
                    &mut writer,
                    &DataMsg::HelloAck(HelloAck {
                        proto_version: PROTO_VERSION,
                        server_build_id: cfg.server_build_id.clone(),
                        workspace_revision: 42,
                        server_pid: cfg.server_pid,
                    }),
                )?;
                continue;
            }

            if let Some(handler) = &cfg.data {
                for reply in handler(&msg) {
                    send(&mut writer, &reply)?;
                }
            }
        }
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        // Ask the accept loop to stop, then wake it with one throwaway connect.
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.socket);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}
