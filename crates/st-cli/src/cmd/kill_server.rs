//! `st kill-server` — graceful shutdown, with a lockfile fallback.
//!
//! The normal path is CONTROL `server.shutdown` (`02-protocol.md` §3.3), which
//! "refuses if surfaces exist unless force". `--force` sets that flag *and*
//! falls back to `SIGTERM` on the pid in the lockfile beside the socket when
//! the server does not answer at all (`03-server.md` §2: the daemon writes its
//! pid into the flock'd lockfile, and SIGTERM is its graceful path).
//!
//! The signal is delivered with `kill(1)` rather than `libc::kill`, so this
//! crate needs neither a `libc` dependency nor an `unsafe` block.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use st_proto::control::Req;

use crate::control::ControlClient;
use crate::exit::{CliError, ExitCode, Result};
use crate::transport::Connector;

/// Runs the command.
pub fn run(
    connector: &dyn Connector,
    lock_path: &Path,
    force: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let socket = connector.describe();

    match ControlClient::connect(connector) {
        Ok(mut client) => {
            let pid = client.hello_ack().server_pid;
            match client.request_raw(|id| Req::ServerShutdown {
                id,
                force: force.then_some(true),
            }) {
                Ok(_) => {
                    writeln!(out, "server {pid} is shutting down").map_err(write_error)?;
                    Ok(())
                }
                // The server closing the connection *is* the shutdown; a
                // graceful daemon may not manage to answer first.
                Err(err) if err.exit == ExitCode::NoServer => {
                    writeln!(out, "server {pid} is shutting down").map_err(write_error)?;
                    Ok(())
                }
                Err(err) if force => {
                    tracing::debug!(%err, "graceful shutdown failed; falling back to SIGTERM");
                    sigterm_from_lockfile(lock_path, out)
                }
                Err(err) => Err(err.with_hint("pass --force to shut down anyway")),
            }
        }
        Err(err) if force => {
            tracing::debug!(%err, "cannot reach {socket}; falling back to SIGTERM");
            sigterm_from_lockfile(lock_path, out)
        }
        Err(err) => Err(err),
    }
}

/// Reads the pid out of the lockfile and sends it `SIGTERM`.
fn sigterm_from_lockfile(lock_path: &Path, out: &mut dyn Write) -> Result<()> {
    let pid = read_pid(lock_path)?;
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .map_err(|e| CliError::failure(format!("cannot run kill(1): {e}")))?;
    if status.success() {
        writeln!(out, "sent SIGTERM to pid {pid}").map_err(write_error)
    } else {
        Err(
            CliError::failure(format!("kill -TERM {pid} failed with {status}"))
                .with_hint("the server may already be gone; remove the stale socket and lockfile"),
        )
    }
}

/// Parses the pid the daemon wrote into its lockfile (`03-server.md` §2).
pub fn read_pid(lock_path: &Path) -> Result<u32> {
    let text = std::fs::read_to_string(lock_path).map_err(|e| {
        CliError::no_server(format!("cannot read {}: {e}", lock_path.display()))
            .with_hint("no lockfile, so no server has run from this runtime directory")
    })?;
    parse_pid(&text)
        .ok_or_else(|| CliError::failure(format!("{} does not contain a pid", lock_path.display())))
}

/// The lockfile holds a bare decimal pid, possibly with trailing whitespace.
#[must_use]
pub fn parse_pid(text: &str) -> Option<u32> {
    let pid: u32 = text.lines().next()?.trim().parse().ok()?;
    (pid > 0).then_some(pid)
}

fn write_error(err: std::io::Error) -> CliError {
    CliError::failure(format!("cannot write output: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lockfile_pid_parses_with_or_without_a_newline() {
        assert_eq!(parse_pid("4242"), Some(4242));
        assert_eq!(parse_pid("4242\n"), Some(4242));
        assert_eq!(parse_pid("  4242  \nignored\n"), Some(4242));
    }

    #[test]
    fn junk_and_zero_are_rejected() {
        assert_eq!(parse_pid(""), None);
        assert_eq!(parse_pid("not a pid"), None);
        assert_eq!(parse_pid("0"), None);
        assert_eq!(parse_pid("-1"), None);
    }

    #[test]
    fn a_missing_lockfile_is_a_no_server_error() {
        let err = read_pid(Path::new("/definitely/not/here/lock")).unwrap_err();
        assert_eq!(err.exit, ExitCode::NoServer);
        assert!(err.hint.unwrap().contains("no lockfile"));
    }

    #[test]
    fn a_lockfile_without_a_pid_is_a_plain_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        std::fs::write(&path, "\n").unwrap();
        let err = read_pid(&path).unwrap_err();
        assert_eq!(err.exit, ExitCode::Failure);
        assert!(err.message.contains("does not contain a pid"));
    }
}
