//! Exit codes and the error type that carries them.
//!
//! Every failure path maps onto one of [`ExitCode`]'s variants so that scripts
//! can branch on the *kind* of failure without parsing stderr. The table is
//! reproduced in `st --help`.

use std::fmt;

/// Process exit codes. Documented in `st --help`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ExitCode {
    /// The command did what was asked.
    Success = 0,
    /// Anything that is not one of the more specific codes below: a file could
    /// not be read, the config is invalid, …
    Failure = 1,
    /// Reserved for clap: the command line itself was wrong. `clap` exits with
    /// this on its own; nothing in this crate returns it.
    Usage = 2,
    /// No server is listening on the socket, or the connection dropped.
    NoServer = 3,
    /// The peer spoke, but not the protocol: a `reject`, a malformed frame, an
    /// unexpected message, a version mismatch.
    Protocol = 4,
    /// The server answered `not_found`: the session, tab or surface does not
    /// exist.
    NotFound = 5,
    /// The server answered an error other than `not_found`, or refused a
    /// graceful shutdown.
    Refused = 6,
}

impl ExitCode {
    /// The numeric value handed to the OS.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// The block of text appended to `st --help` describing [`ExitCode`].
pub const EXIT_CODE_HELP: &str = "\
Exit codes:
  0  success
  1  failure (I/O, bad file, invalid config)
  2  usage error (bad arguments)
  3  no server (nothing listening on the socket)
  4  protocol error (handshake rejected, undecodable frame)
  5  not found (unknown session, tab or surface id)
  6  refused (the server answered with an error)";

/// An error that knows which [`ExitCode`] it should produce.
#[derive(Debug, thiserror::Error)]
pub struct CliError {
    /// The exit code to report.
    pub exit: ExitCode,
    /// The message printed to stderr, prefixed with `st: `.
    pub message: String,
    /// Optional extra context printed on its own line, indented.
    pub hint: Option<String>,
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, "\n  {hint}")?;
        }
        Ok(())
    }
}

impl CliError {
    /// Builds an error with the given code and message.
    pub fn new(exit: ExitCode, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
            hint: None,
        }
    }

    /// Attaches a hint line, shown indented under the message.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// [`ExitCode::NoServer`].
    pub fn no_server(message: impl Into<String>) -> Self {
        Self::new(ExitCode::NoServer, message)
    }

    /// [`ExitCode::Protocol`].
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(ExitCode::Protocol, message)
    }

    /// [`ExitCode::Failure`].
    pub fn failure(message: impl Into<String>) -> Self {
        Self::new(ExitCode::Failure, message)
    }
}

/// The result type every command returns.
pub type Result<T, E = CliError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(ExitCode::Success.code(), 0);
        assert_eq!(ExitCode::NoServer.code(), 3);
        assert_eq!(ExitCode::Protocol.code(), 4);
        assert_eq!(ExitCode::NotFound.code(), 5);
        assert_eq!(ExitCode::Refused.code(), 6);
    }

    #[test]
    fn hints_render_on_their_own_line() {
        let err = CliError::no_server("no server").with_hint("start one");
        assert_eq!(err.to_string(), "no server\n  start one");
        assert_eq!(err.exit, ExitCode::NoServer);
    }

    #[test]
    fn help_text_lists_every_code() {
        for code in ["0", "1", "2", "3", "4", "5", "6"] {
            assert!(
                EXIT_CODE_HELP.contains(&format!("  {code}  ")),
                "exit code {code} is missing from the help text"
            );
        }
    }
}
