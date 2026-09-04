//! `superterminald` — the Superterminal daemon.
//!
//! ```text
//! superterminald [--foreground] [--daemonize] [--socket <path>] [--tcp <addr>]
//!                [--config <path>] [--state-dir <path>] [--no-idle-exit] [-v|-vv]
//! ```
//!
//! The client normally spawns the daemon detached (grilling Q30), so
//! `--daemonize` exists mainly for shell users: it re-executes this binary in
//! the background and returns. Everything else runs the daemon in this
//! process; `--foreground` only adds a stderr log layer.

use std::path::PathBuf;

use clap::Parser;
use st_server::lifecycle::{self, Options};

/// The Superterminal daemon: owns every PTY, the Workspace and one Unix socket.
#[derive(Debug, Parser)]
#[command(name = "superterminald", version, about, long_about = None)]
struct Cli {
    /// Log to stderr as well as to the rolling log file.
    #[arg(long)]
    foreground: bool,

    /// Re-exec in the background and return immediately.
    #[arg(long, conflicts_with = "foreground")]
    daemonize: bool,

    /// Listen on this socket instead of the default; moves the lock file with it.
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,

    /// Also listen on loopback TCP (e.g. `--tcp 127.0.0.1:7171`) for the
    /// Windows client: a socket file cannot cross the WSL boundary, shared
    /// localhost TCP can. Only loopback addresses are accepted.
    #[arg(long, value_name = "ADDR")]
    tcp: Option<std::net::SocketAddr>,

    /// Read this `config.toml` instead of the default.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Keep `workspace.json` and the logs here instead of the default.
    #[arg(long, value_name = "PATH")]
    state_dir: Option<PathBuf>,

    /// Never exit when idle, whatever `[server].idle_exit_minutes` says.
    #[arg(long)]
    no_idle_exit: bool,

    /// Increase log verbosity (`-v` = debug, `-vv` = trace).
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

impl Cli {
    fn options(&self) -> Options {
        Options {
            socket: self.socket.clone(),
            tcp: self.tcp,
            config: self.config.clone(),
            state_dir: self.state_dir.clone(),
            foreground: self.foreground,
            no_idle_exit: self.no_idle_exit,
            verbosity: self.verbose,
        }
    }
}

/// Set in the child so a re-exec cannot recurse.
const DAEMONIZED: &str = "SUPERTERMINAL_DAEMONIZED";

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.daemonize && std::env::var_os(DAEMONIZED).is_none() {
        return respawn_detached(&cli);
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("superterminald")
        .build()?
        .block_on(lifecycle::run(cli.options()))
}

/// Restarts this binary in the background and returns.
///
/// This is deliberately *not* a double `fork`: `03-server.md` §2 says the
/// server never daemonises itself — the client spawns it detached — so all
/// this needs to do is survive the shell that typed the command. The child
/// inherits no terminal (its stdio is `/dev/null`) and installs a `SIGHUP`
/// handler that reloads the configuration instead of dying, which is what
/// would otherwise kill it when the terminal closes.
fn respawn_detached(cli: &Cli) -> anyhow::Result<()> {
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe()?;
    let mut child = Command::new(exe);
    child
        .env(DAEMONIZED, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Some(path) = &cli.socket {
        child.arg("--socket").arg(path);
    }
    if let Some(addr) = &cli.tcp {
        child.arg("--tcp").arg(addr.to_string());
    }
    if let Some(path) = &cli.config {
        child.arg("--config").arg(path);
    }
    if let Some(path) = &cli.state_dir {
        child.arg("--state-dir").arg(path);
    }
    if cli.no_idle_exit {
        child.arg("--no-idle-exit");
    }
    for _ in 0..cli.verbose {
        child.arg("-v");
    }

    let spawned = child.spawn()?;
    println!("superterminald started (pid {})", spawned.id());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cli_parses_every_flag() {
        let cli = Cli::try_parse_from([
            "superterminald",
            "--foreground",
            "--socket",
            "/tmp/x.sock",
            "--tcp",
            "127.0.0.1:7171",
            "--config",
            "/tmp/config.toml",
            "--state-dir",
            "/tmp/state",
            "--no-idle-exit",
            "-vv",
        ])
        .unwrap();

        let options = cli.options();
        assert!(options.foreground);
        assert!(options.no_idle_exit);
        assert_eq!(options.verbosity, 2);
        assert_eq!(options.socket.unwrap(), PathBuf::from("/tmp/x.sock"));
        assert_eq!(options.tcp.unwrap().to_string(), "127.0.0.1:7171");
        assert_eq!(options.config.unwrap(), PathBuf::from("/tmp/config.toml"));
        assert_eq!(options.state_dir.unwrap(), PathBuf::from("/tmp/state"));
    }

    #[test]
    fn foreground_and_daemonize_are_mutually_exclusive() {
        assert!(Cli::try_parse_from(["superterminald", "--foreground", "--daemonize"]).is_err());
    }

    #[test]
    fn defaults_are_all_off() {
        let cli = Cli::try_parse_from(["superterminald"]).unwrap();
        let options = cli.options();
        assert!(!options.foreground);
        assert_eq!(options.verbosity, 0);
        assert!(options.socket.is_none());
    }
}
