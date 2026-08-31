//! Test harness: a whole daemon in-process on a temporary socket, plus a raw
//! NDJSON client that speaks the wire protocol and nothing else.
//!
//! Nothing here uses `st-server`'s own client code, so the tests exercise the
//! bytes a real client would send (`docs/plan/02-protocol.md` §1–§3).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use st_config::{Config, Paths, Platform};
use st_server::lifecycle::{RunningServer, ServerBuilder};
use st_server::workspace::NullSpawner;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

/// How long any single read is allowed to take before the test fails.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// A running daemon plus the temporary directory holding its socket, lock and
/// `workspace.json`.
pub struct Harness {
    pub dir: TempDir,
    pub paths: Paths,
    pub spawner: Arc<NullSpawner>,
    pub server: Option<RunningServer>,
    config: Config,
    debounce: Duration,
}

/// Builds [`Paths`] that touch nothing outside `dir`.
pub fn paths_in(dir: &Path) -> Paths {
    let runtime = dir.join("run").into_os_string();
    let state = dir.join("state").into_os_string();
    Paths::from_lookup(
        Platform::Linux,
        st_config::current_uid(),
        move |key| match key {
            "SUPERTERMINAL_RUNTIME_DIR" => Some(runtime.clone()),
            "SUPERTERMINAL_STATE_DIR" => Some(state.clone()),
            _ => None,
        },
    )
}

impl Harness {
    /// Starts a daemon with the default configuration.
    pub async fn start() -> Self {
        Self::start_with(Config::default()).await
    }

    /// Starts a daemon with a specific configuration.
    pub async fn start_with(config: Config) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        Self::start_in(dir, config, Duration::from_millis(80)).await
    }

    /// Starts a daemon in an existing directory, so a restart can reuse the
    /// same `workspace.json`.
    pub async fn start_in(dir: TempDir, config: Config, debounce: Duration) -> Self {
        let paths = paths_in(dir.path());
        let spawner = Arc::new(NullSpawner::new());
        let server = ServerBuilder::new(paths.clone(), config.clone())
            .spawner(spawner.clone())
            .persist_debounce(debounce)
            .build_id("test-build")
            .start()
            .await
            .expect("the daemon starts");
        Self {
            dir,
            paths,
            spawner,
            server: Some(server),
            config,
            debounce,
        }
    }

    /// The socket clients dial.
    pub fn socket(&self) -> PathBuf {
        self.paths.socket_path()
    }

    /// The persisted document's path.
    pub fn workspace_file(&self) -> PathBuf {
        self.paths.workspace_file().expect("state dir")
    }

    /// The `workspace.json` contents, or `None` when it has not been written.
    pub fn saved(&self) -> Option<Value> {
        let text = std::fs::read_to_string(self.workspace_file()).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// A connected, handshaken control client.
    pub async fn client(&self) -> Client {
        let mut client = Client::connect(&self.socket()).await;
        let ack = client.hello("1.0").await;
        assert_eq!(ack["t"], "hello.ack", "handshake failed: {ack}");
        client
    }

    /// Stops the daemon, keeping the directory (so a restart can follow).
    pub async fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.stop("test").await.expect("clean shutdown");
        }
    }

    /// Stops and starts again from the same state directory.
    pub async fn restart(mut self) -> Self {
        self.stop().await;
        let Harness {
            dir,
            config,
            debounce,
            ..
        } = self;
        Self::start_in(dir, config, debounce).await
    }

    /// Waits until `predicate` holds or the timeout expires.
    pub async fn wait_until(&self, timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if predicate() {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// A raw NDJSON control client.
pub struct Client {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: u32,
    events: Vec<Value>,
}

impl Client {
    /// Connects without handshaking.
    pub async fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket)
            .await
            .unwrap_or_else(|e| panic!("cannot connect to {}: {e}", socket.display()));
        let (read, write) = stream.into_split();
        Self {
            reader: BufReader::new(read),
            writer: write,
            next_id: 1,
            events: Vec::new(),
        }
    }

    /// Sends a `hello` at `version` and returns the server's answer.
    pub async fn hello(&mut self, version: &str) -> Value {
        self.send(json!({
            "t": "hello",
            "proto_version": version,
            "client_kind": "tool",
            "build_id": "test-client",
        }))
        .await;
        self.read_line().await.expect("a handshake answer")
    }

    /// Writes one JSON value as a line.
    pub async fn send(&mut self, value: Value) {
        let mut line = value.to_string();
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("write to the server");
    }

    /// Writes raw bytes, for the malformed-input tests.
    pub async fn send_raw(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).await.expect("write raw bytes");
    }

    /// Reads one line, whatever it is. `None` at EOF.
    pub async fn read_line(&mut self) -> Option<Value> {
        let mut line = String::new();
        let read = tokio::time::timeout(READ_TIMEOUT, self.reader.read_line(&mut line))
            .await
            .expect("the server answers within the timeout")
            .expect("the socket is readable");
        if read == 0 {
            return None;
        }
        Some(serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad line {line:?}: {e}")))
    }

    /// Sends a request with the next id and returns its response, queueing any
    /// events that arrive first.
    pub async fn request(&mut self, mut req: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        req["id"] = json!(id);
        self.send(req).await;

        loop {
            let value = self.read_line().await.expect("a response");
            let tag = value["t"].as_str().unwrap_or_default();
            if tag.starts_with("ev.") {
                self.events.push(value);
                continue;
            }
            assert_eq!(value["id"], json!(id), "response out of order: {value}");
            return value;
        }
    }

    /// [`Client::request`], asserting success and returning `result`.
    pub async fn ok(&mut self, req: Value) -> Value {
        let res = self.request(req).await;
        assert_eq!(res["t"], "ok", "expected ok, got {res}");
        res["result"].clone()
    }

    /// [`Client::request`], asserting failure and returning the error code.
    pub async fn err(&mut self, req: Value) -> String {
        let res = self.request(req).await;
        assert_eq!(res["t"], "err", "expected err, got {res}");
        res["error"]["code"]
            .as_str()
            .expect("an error code")
            .to_string()
    }

    /// The next event, from the queue or from the socket.
    pub async fn next_event(&mut self) -> Value {
        if !self.events.is_empty() {
            return self.events.remove(0);
        }
        loop {
            let value = self.read_line().await.expect("an event");
            if value["t"].as_str().unwrap_or_default().starts_with("ev.") {
                return value;
            }
        }
    }

    /// The next `ev.workspace`, skipping anything else.
    pub async fn next_workspace_event(&mut self) -> Value {
        loop {
            let event = self.next_event().await;
            if event["t"] == "ev.workspace" {
                return event;
            }
        }
    }

    /// `true` when nothing arrives within `window`.
    pub async fn is_quiet_for(&mut self, window: Duration) -> bool {
        if !self.events.is_empty() {
            return false;
        }
        let mut line = String::new();
        tokio::time::timeout(window, self.reader.read_line(&mut line))
            .await
            .is_err()
    }

    /// Closes the connection.
    pub async fn close(mut self) {
        let _ = self.writer.shutdown().await;
    }
}

/// The `workspace.subscribe` request.
pub fn subscribe() -> Value {
    json!({ "t": "workspace.subscribe" })
}

/// A `spawn` block for `tab.create` / `surface.create`.
pub fn spawn_spec() -> Value {
    json!({ "cols": 100, "rows": 30 })
}
