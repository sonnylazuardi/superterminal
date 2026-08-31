//! A standalone Data Plane *server* stub, for tests that need a real socket
//! but no daemon.
//!
//! `<terminal-grid socketPath=...>` is only interesting once it has a
//! `Snapshot` to paint, and the real server drags in a PTY, a VT parser and a
//! Workspace. This example speaks just enough of `02-protocol.md` §2, §4 and
//! §7 to get a Replica populated with known text, so a Bun integration test
//! can assert on what the element renders.
//!
//! ```text
//! cargo run --example fake_dataplane -- /tmp/st-test.sock \
//!     --cols 80 --rows 24 --surface 1 --text "line one|line two"
//! ```
//!
//! It prints `READY <path>` on stdout once the listener is bound — a harness
//! should wait for that line rather than poll for the socket file, because
//! `bind` creates the file before `listen` makes it connectable.
//!
//! What it implements: `Hello` → `HelloAck`, `Attach` → `Snapshot`, `Resize` →
//! a resizing `Delta`, `FetchHistory` → an empty `History`. Everything else is
//! accepted and dropped, which keeps `Input` and `Ack` from ending the
//! connection.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use st_proto::{
    AbsLine, Cursor, DataError, DataMsg, Delta, DirtyRow, FrameDecoder, HelloAck, History, Modes,
    PackedCell, Row, Seq, Snapshot, Style, StyleIdx, SurfaceId, ViewState, DATA_ERR_NOT_ATTACHED,
    PROTO_VERSION,
};

/// The `--help` text, also printed on a bad argument.
const USAGE: &str = "\
usage: fake_dataplane <socket-path> [options]

options:
  --cols <n>            grid width in columns (default 80)
  --rows <n>            grid height in rows (default 24)
  --surface <id>        the surface id this stub serves (default 1)
  --text <a|b|c>        '|'-separated lines to fill the grid with
  --exit-after-ms <n>   exit this many milliseconds after binding
  -h, --help            print this text";

/// What the stub was told to pretend to be.
#[derive(Debug, Clone)]
struct Config {
    /// Where to bind the listener.
    socket_path: PathBuf,
    /// Grid width reported in the `Snapshot`.
    cols: u16,
    /// Grid height reported in the `Snapshot`.
    rows: u16,
    /// The only surface this stub knows about.
    surface_id: SurfaceId,
    /// The lines to write into the top of the grid.
    lines: Vec<String>,
    /// Shut the process down this long after binding, if set.
    exit_after_ms: Option<u64>,
}

impl Config {
    /// Parses the command line, or returns a message to show the user.
    fn from_args(args: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let args: Vec<String> = args.into_iter().collect();
        let mut socket_path: Option<PathBuf> = None;
        let mut cols = 80u16;
        let mut rows = 24u16;
        let mut surface_id = SurfaceId(1);
        let mut text = String::from("fake dataplane");
        let mut exit_after_ms = None;

        // Every option below takes exactly one value, hence the shared
        // lookahead and the `i += 2`.
        let mut i = 0;
        while i < args.len() {
            let arg = args[i].as_str();
            match arg {
                "-h" | "--help" => return Ok(None),
                "--cols" => {
                    cols = parse_u16(&value_of(&args, i, arg)?, arg)?;
                    i += 2;
                }
                "--rows" => {
                    rows = parse_u16(&value_of(&args, i, arg)?, arg)?;
                    i += 2;
                }
                "--surface" => {
                    let raw = value_of(&args, i, arg)?;
                    let id = raw
                        .parse::<u32>()
                        .map_err(|err| format!("--surface: {raw:?} is not a surface id: {err}"))?;
                    surface_id = SurfaceId(id);
                    i += 2;
                }
                "--text" => {
                    text = value_of(&args, i, arg)?;
                    i += 2;
                }
                "--exit-after-ms" => {
                    let raw = value_of(&args, i, arg)?;
                    let ms = raw.parse::<u64>().map_err(|err| {
                        format!("--exit-after-ms: {raw:?} is not a number: {err}")
                    })?;
                    exit_after_ms = Some(ms);
                    i += 2;
                }
                other if other.starts_with('-') => return Err(format!("unknown option {other:?}")),
                other if socket_path.is_none() => {
                    socket_path = Some(PathBuf::from(other));
                    i += 1;
                }
                other => return Err(format!("unexpected argument {other:?}")),
            }
        }

        let socket_path = socket_path.ok_or_else(|| "a socket path is required".to_string())?;
        Ok(Some(Self {
            socket_path,
            // A zero-sized grid would make every row index out of range, and
            // the real server never reports one.
            cols: cols.max(1),
            rows: rows.max(1),
            surface_id,
            lines: text.split('|').map(str::to_string).collect(),
            exit_after_ms,
        }))
    }
}

/// The value that follows the flag at `index`.
fn value_of(args: &[String], index: usize, flag: &str) -> Result<String, String> {
    args.get(index + 1)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Parses a grid dimension.
fn parse_u16(raw: &str, flag: &str) -> Result<u16, String> {
    raw.parse::<u16>()
        .map_err(|err| format!("{flag}: {raw:?} is not a number: {err}"))
}

fn main() -> ExitCode {
    let config = match Config::from_args(std::env::args().skip(1)) {
        Ok(Some(config)) => config,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("fake_dataplane: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    match serve(&config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("fake_dataplane: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Binds the listener and serves connections until the process is killed.
fn serve(config: &Config) -> std::io::Result<()> {
    // A socket file left behind by a killed run makes `bind` fail with
    // EADDRINUSE, which would strand every later test on the same path.
    match std::fs::remove_file(&config.socket_path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    let listener = UnixListener::bind(&config.socket_path)?;

    // The harness blocks on this line: the socket file exists from `bind`
    // onwards, but only a listening socket accepts a connect.
    println!("READY {}", config.socket_path.display());
    std::io::stdout().flush()?;

    if let Some(ms) = config.exit_after_ms {
        let path = config.socket_path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            let _ = std::fs::remove_file(&path);
            std::process::exit(0);
        });
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // One thread per connection: the stub only ever answers
                // requests, so blocking reads need no runtime.
                let config = config.clone();
                std::thread::spawn(move || {
                    if let Err(err) = serve_connection(stream, &config) {
                        eprintln!("fake_dataplane: connection failed: {err}");
                    }
                });
            }
            Err(err) => eprintln!("fake_dataplane: accept failed: {err}"),
        }
    }

    Ok(())
}

/// Reads one client's frames and answers them until it goes away.
///
/// A client that disconnects is normal, not an error: the function returns
/// `Ok` for a clean EOF and for a reset or broken pipe.
fn serve_connection(mut stream: UnixStream, config: &Config) -> std::io::Result<()> {
    // Per-connection state. Two clients resizing the same surface would fight
    // over a shared one, and no test needs them to agree.
    let mut surface = FakeSurface::new(config);
    let mut decoder = FrameDecoder::expecting_magic();
    let mut buf = [0u8; 8192];

    loop {
        let read = match stream.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) if is_disconnect(&err) => return Ok(()),
            Err(err) => return Err(err),
        };
        decoder.push(&buf[..read]);

        loop {
            let frame = match decoder.next_frame() {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(err) => {
                    // Framing errors poison the decoder, so the only thing
                    // left to do is drop the connection.
                    eprintln!("fake_dataplane: framing error: {err}");
                    return Ok(());
                }
            };
            let msg = match DataMsg::from_frame(frame.msg_type, &frame.payload) {
                Ok(msg) => msg,
                Err(err) => {
                    eprintln!("fake_dataplane: undecodable frame: {err}");
                    continue;
                }
            };
            for reply in surface.respond_to(&msg) {
                match write_msg(&mut stream, &reply) {
                    Ok(()) => {}
                    Err(err) if is_disconnect(&err) => return Ok(()),
                    Err(err) => return Err(err),
                }
            }
        }
    }
}

/// `true` for the errors a client hanging up produces.
fn is_disconnect(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::ConnectionReset | ErrorKind::BrokenPipe | ErrorKind::UnexpectedEof
    )
}

/// Frames one message and writes it.
fn write_msg(stream: &mut UnixStream, msg: &DataMsg) -> std::io::Result<()> {
    let mut out = Vec::new();
    msg.encode_to(&mut out)
        .map_err(|err| std::io::Error::other(format!("encoding {:#06x}: {err}", msg.msg_type())))?;
    stream.write_all(&out)
}

/// The one surface this stub serves, as seen by one connection.
struct FakeSurface {
    /// The id clients must attach to.
    surface_id: SurfaceId,
    /// Current grid width; `Resize` moves it.
    cols: u16,
    /// Current grid height; `Resize` moves it.
    rows: u16,
    /// The sequence number of the state described by the last message sent.
    seq: Seq,
    /// The canned text, one entry per grid row from the top.
    lines: Vec<String>,
}

impl FakeSurface {
    /// A surface at [`Seq::FIRST`], the state a freshly created one is in.
    fn new(config: &Config) -> Self {
        Self {
            surface_id: config.surface_id,
            cols: config.cols,
            rows: config.rows,
            seq: Seq::FIRST,
            lines: config.lines.clone(),
        }
    }

    /// The replies one client message earns, in send order.
    fn respond_to(&mut self, msg: &DataMsg) -> Vec<DataMsg> {
        match msg {
            DataMsg::Hello(_) => vec![DataMsg::HelloAck(HelloAck {
                proto_version: PROTO_VERSION,
                server_build_id: "fake_dataplane".to_string(),
                workspace_revision: 0,
                server_pid: std::process::id(),
            })],
            // `want_snapshot` is ignored: a stub with one fixed state has
            // nothing cheaper to send, and always snapshotting keeps a
            // reconnecting client correct.
            DataMsg::Attach(attach) => match self.check(attach.surface_id) {
                Ok(()) => vec![DataMsg::Snapshot(Box::new(self.snapshot()))],
                Err(err) => vec![err],
            },
            DataMsg::Resize(resize) => match self.check(resize.surface_id) {
                Ok(()) => vec![DataMsg::Delta(Box::new(
                    self.resize(resize.cols.max(1), resize.rows.max(1)),
                ))],
                Err(err) => vec![err],
            },
            DataMsg::FetchHistory(fetch) => match self.check(fetch.surface_id) {
                Ok(()) => vec![DataMsg::History(Box::new(History {
                    surface_id: self.surface_id,
                    // There is no scrollback, so the answer starts at the
                    // trim point whatever was asked for.
                    from_line: AbsLine(0),
                    history_base: AbsLine(0),
                    rows: Vec::new(),
                }))],
                Err(err) => vec![err],
            },
            // `Input`, `Ack`, `Detach`, `SetViewState` and anything from a
            // newer minor: accepted, unanswered, connection kept.
            _ => Vec::new(),
        }
    }

    /// Rejects a message aimed at a surface this stub does not serve, which is
    /// otherwise a silent hang in the test that made the typo.
    fn check(&self, surface_id: SurfaceId) -> Result<(), DataMsg> {
        if surface_id == self.surface_id {
            return Ok(());
        }
        Err(DataMsg::DataError(DataError {
            surface_id: Some(surface_id),
            code: DATA_ERR_NOT_ATTACHED,
            message: format!("fake_dataplane only serves surface {}", self.surface_id),
        }))
    }

    /// The full grid: the canned lines from the top, blanks below.
    fn grid(&self) -> Vec<Row> {
        (0..usize::from(self.rows))
            .map(|index| match self.lines.get(index) {
                Some(line) => row_of(line, self.cols),
                None => Row::new(),
            })
            .collect()
    }

    /// A cursor parked just past the canned text.
    fn cursor(&self) -> Cursor {
        let last = self.rows.saturating_sub(1);
        let row = u16::try_from(self.lines.len()).unwrap_or(last).min(last);
        Cursor {
            row,
            col: 0,
            ..Cursor::default()
        }
    }

    /// The current state as a `Snapshot`.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            surface_id: self.surface_id,
            seq: self.seq,
            cols: self.cols,
            rows: self.rows,
            styles: vec![Style::DEFAULT],
            grid: self.grid(),
            cursor: self.cursor(),
            modes: Modes::empty(),
            title: "fake".to_string(),
            history_base: AbsLine(0),
            history_len: 0,
            view_state: ViewState::default(),
            exited: None,
        }
    }

    /// Applies a resize and describes it as a `Delta`.
    ///
    /// Every row is re-sent: the replica resizes before applying rows and does
    /// not reflow, so anything not marked dirty would be left blank.
    fn resize(&mut self, cols: u16, rows: u16) -> Delta {
        self.cols = cols;
        self.rows = rows;
        let since_seq = self.seq;
        self.seq = self.seq.next();

        Delta {
            surface_id: self.surface_id,
            seq: self.seq,
            since_seq,
            history_base: AbsLine(0),
            history_len: 0,
            resized: Some((cols, rows)),
            new_styles: Vec::new(),
            rows: self
                .grid()
                .into_iter()
                .enumerate()
                .map(|(index, row)| DirtyRow {
                    index: u16::try_from(index).unwrap_or(u16::MAX),
                    row,
                })
                .collect(),
            cursor: self.cursor(),
            modes: Modes::empty(),
            title: None,
        }
    }
}

/// Builds a default-styled row from `text`, clipped to `cols`.
///
/// Trailing blanks are trimmed because the wire format says a sender must trim
/// them; the receiver re-pads (§4.4).
fn row_of(text: &str, cols: u16) -> Row {
    let mut row = Row {
        cells: text
            .chars()
            .take(usize::from(cols))
            .map(|ch| PackedCell::from_char(ch, StyleIdx::ZERO))
            .collect(),
        extras: Vec::new(),
        wrapped: false,
    };
    row.trim_trailing_blanks();
    row
}
