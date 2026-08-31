//! `st` — the Superterminal control and inspection CLI (grilling Q46).
//!
//! The binary is a thin `main.rs`; everything worth testing lives here:
//!
//! * [`transport`] — the injectable byte-stream abstraction plus the Unix-socket
//!   implementation and socket-path resolution (`--socket`,
//!   `$SUPERTERMINAL_SOCKET`, `st-config`).
//! * [`control`] — the CONTROL plane: newline-delimited JSON, the
//!   `hello`/`hello.ack` handshake and request/response correlation
//!   (`docs/plan/02-protocol.md` §1.2, §2, §3).
//! * [`dataplane`] — the DATA plane: the `0xFF "STD"` magic, the postcard
//!   handshake and `Attach` (§1.3, §2, §4).
//! * [`replica`] — the client-side grid replica: `Snapshot` → state,
//!   `Delta` → state, gap detection (§6, §7).
//! * [`render`] — grid → plain text or ANSI SGR, with the trailing-blank
//!   trimming and wide-cell rules of §4.4 / §5.1.
//! * [`cmd`] — one module per subcommand.
//!
//! Nothing here needs a running server: every entry point takes a
//! [`transport::Connector`], so the integration tests in `tests/` drive the
//! real codecs over a temp Unix socket served by a fake server.

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod cli;
pub mod cmd;
pub mod control;
pub mod dataplane;
pub mod exit;
pub mod render;
pub mod replica;
pub mod transport;

pub use cli::Cli;
pub use exit::{CliError, ExitCode};

/// The `build_id` this CLI reports in its `Hello` (§2, rule 4: informational
/// only, never used for decisions).
///
/// `SUPERTERMINAL_BUILD_ID` is baked in by the build when available; otherwise
/// the crate version stands in.
#[must_use]
pub fn build_id() -> String {
    option_env!("SUPERTERMINAL_BUILD_ID").map_or_else(
        || format!("st-cli {}", env!("CARGO_PKG_VERSION")),
        Into::into,
    )
}
