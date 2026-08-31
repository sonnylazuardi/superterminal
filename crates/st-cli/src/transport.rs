//! Byte-stream transport and socket-path resolution.
//!
//! Both planes are just a bidirectional byte stream ("the framing is
//! transport-agnostic", `docs/plan/02-protocol.md` §1.1), so every client in
//! this crate is written against [`Transport`] rather than against
//! [`std::os::unix::net::UnixStream`]. A [`Connector`] produces one; the real
//! CLI uses [`UnixConnector`], and tests can hand in anything else.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::exit::{CliError, ExitCode};

/// A bidirectional byte stream that can be split into an owned reader and an
/// owned writer.
///
/// The split matters for `st probe --follow`: one thread blocks on reads while
/// the main thread still needs to send `Ack` frames.
pub trait Transport: Read + Write + Send {
    /// Returns an independent handle onto the same stream.
    fn try_clone_box(&self) -> io::Result<Box<dyn Transport>>;

    /// Sets a read timeout; `None` blocks forever.
    fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()>;

    /// Closes the write half, signalling EOF to the peer.
    fn shutdown_write(&self) -> io::Result<()>;
}

impl Transport for UnixStream {
    fn try_clone_box(&self) -> io::Result<Box<dyn Transport>> {
        Ok(Box::new(self.try_clone()?))
    }

    fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        UnixStream::set_read_timeout(self, dur)
    }

    fn shutdown_write(&self) -> io::Result<()> {
        self.shutdown(std::net::Shutdown::Write)
    }
}

/// Opens a [`Transport`] to the server.
pub trait Connector {
    /// Connects, or fails with an error that already carries the right exit
    /// code.
    fn connect(&self) -> Result<Box<dyn Transport>, CliError>;

    /// The path this connector will dial, for messages.
    fn describe(&self) -> &Path;
}

/// The production connector: a Unix domain socket (§1.1).
#[derive(Debug, Clone)]
pub struct UnixConnector {
    path: PathBuf,
}

impl UnixConnector {
    /// Dials `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl Connector for UnixConnector {
    fn connect(&self) -> Result<Box<dyn Transport>, CliError> {
        match UnixStream::connect(&self.path) {
            Ok(stream) => {
                // Never hang forever on a server that accepted the connection
                // and then went silent; the handshake budget is 5 s (§2).
                let _ = Transport::set_read_timeout(&stream, Some(Duration::from_secs(30)));
                Ok(Box::new(stream))
            }
            Err(err) => Err(connect_error(&self.path, &err)),
        }
    }

    fn describe(&self) -> &Path {
        &self.path
    }
}

/// Turns a `connect(2)` failure into a diagnosis the user can act on.
fn connect_error(path: &Path, err: &io::Error) -> CliError {
    let shown = path.display();
    match err.kind() {
        io::ErrorKind::NotFound => CliError::no_server(format!("no server socket at {shown}"))
            .with_hint("no superterminald is running; start the app, or pass --socket <path>"),
        io::ErrorKind::ConnectionRefused => CliError::no_server(format!(
            "nothing is listening on {shown}"
        ))
        .with_hint("a stale socket is left over from a dead server; remove it and start a new one"),
        io::ErrorKind::PermissionDenied => {
            CliError::new(ExitCode::Failure, format!("permission denied on {shown}"))
                .with_hint("the socket belongs to another user; superterminal is per-user")
        }
        _ => CliError::no_server(format!("cannot connect to {shown}: {err}")),
    }
}

/// Resolves the socket path in precedence order (§1.1):
///
/// 1. `--socket <path>` on the command line,
/// 2. `$SUPERTERMINAL_SOCKET`,
/// 3. the platform default from `st-config`
///    (`$XDG_RUNTIME_DIR/superterminal/server.sock` on Linux).
///
/// Steps 2 and 3 are both `st_config::socket_path()`, which already honours
/// the environment override.
pub fn resolve_socket_path(flag: Option<&Path>) -> PathBuf {
    match flag {
        Some(path) => path.to_path_buf(),
        None => st_config::socket_path(),
    }
}

/// The lockfile that sits beside the socket and holds the daemon's pid
/// (§1.1, `03-server.md` §2 step 3).
///
/// With `--socket` the lock is the sibling of the given socket, so a test
/// server started with `--socket /tmp/x/sock` is still killable.
pub fn resolve_lock_path(socket_flag: Option<&Path>) -> PathBuf {
    match socket_flag {
        Some(sock) => sock.parent().map_or_else(
            || PathBuf::from(st_config::LOCK_FILE_NAME),
            |dir| dir.join(st_config::LOCK_FILE_NAME),
        ),
        None => st_config::lock_path(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_beats_the_environment_and_the_default() {
        let flag = PathBuf::from("/tmp/explicit.sock");
        assert_eq!(resolve_socket_path(Some(&flag)), flag);
    }

    #[test]
    fn lock_is_the_socket_sibling_when_overridden() {
        let sock = PathBuf::from("/tmp/st-test-42/server.sock");
        assert_eq!(
            resolve_lock_path(Some(&sock)),
            PathBuf::from("/tmp/st-test-42").join(st_config::LOCK_FILE_NAME)
        );
    }

    #[test]
    fn missing_socket_is_a_no_server_error_with_a_hint() {
        let err = connect_error(
            Path::new("/nope/server.sock"),
            &io::Error::from(io::ErrorKind::NotFound),
        );
        assert_eq!(err.exit, ExitCode::NoServer);
        assert!(err.message.contains("/nope/server.sock"));
        assert!(err.hint.unwrap().contains("--socket"));
    }

    #[test]
    fn refused_and_denied_are_diagnosed_differently() {
        let refused = connect_error(
            Path::new("/tmp/s"),
            &io::Error::from(io::ErrorKind::ConnectionRefused),
        );
        assert_eq!(refused.exit, ExitCode::NoServer);
        assert!(refused.hint.unwrap().contains("stale"));

        let denied = connect_error(
            Path::new("/tmp/s"),
            &io::Error::from(io::ErrorKind::PermissionDenied),
        );
        assert_eq!(denied.exit, ExitCode::Failure);
    }

    #[test]
    fn a_unix_stream_pair_is_a_transport() {
        let (a, b) = UnixStream::pair().unwrap();
        let mut a2 = a.try_clone_box().unwrap();
        a2.write_all(b"ping").unwrap();
        a2.flush().unwrap();
        let mut buf = [0u8; 4];
        (&b).read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
        Transport::set_read_timeout(&a, Some(Duration::from_millis(50))).unwrap();
        a.shutdown_write().unwrap();
    }
}
