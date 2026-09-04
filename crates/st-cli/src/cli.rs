//! The `st` command line (clap v4, derive).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::exit::EXIT_CODE_HELP;

/// `st` — inspect and control a running superterminald.
#[derive(Debug, Parser)]
#[command(
    name = "st",
    version,
    about = "Inspect and control a running superterminald",
    long_about = "Inspect and control a running superterminald.\n\n\
                  Every subcommand talks to the server over its Unix socket \
                  (docs/plan/02-protocol.md): `status`, `ls`, `kill-server` and \
                  `config` use the JSON CONTROL plane, `probe` uses the binary \
                  DATA plane. `dump-data` needs no server at all.",
    after_help = EXIT_CODE_HELP,
    after_long_help = EXIT_CODE_HELP,
    propagate_version = true
)]
pub struct Cli {
    /// Path to the server's Unix socket.
    ///
    /// Defaults to $SUPERTERMINAL_SOCKET, then to the platform location
    /// ($XDG_RUNTIME_DIR/superterminal/server.sock on Linux).
    #[arg(long, short = 's', global = true, value_name = "PATH")]
    pub socket: Option<PathBuf>,

    /// Talk to the server over TCP instead (e.g. `--tcp 127.0.0.1:7171`).
    ///
    /// Defaults to $SUPERTERMINAL_TCP. This is the Windows-client/WSL-server
    /// transport: a socket file cannot cross the VM boundary.
    #[arg(long, global = true, value_name = "ADDR")]
    pub tcp: Option<std::net::SocketAddr>,

    /// Log level for st's own diagnostics; overrides $SUPERTERMINAL_LOG.
    #[arg(long, global = true, value_name = "LEVEL")]
    pub log_level: Option<String>,

    /// Shorthand for --log-level=debug.
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands of `st` (grilling Q46).
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show the server's build id, uptime, connection and object counts.
    Status(StatusArgs),

    /// List sessions, tabs and surfaces as a tree.
    Ls(LsArgs),

    /// Attach to a surface on the DATA plane and print its screen.
    ///
    /// This is the M1 acceptance tool: it proves the server produces a correct
    /// Snapshot and Delta stream with no GUI in the picture.
    Probe(ProbeArgs),

    /// Ask the server to shut down.
    KillServer(KillServerArgs),

    /// Decode a recorded DATA-plane stream and print one line per frame.
    ///
    /// Pairs with SUPERTERMINAL_RECORD=1, which makes the server write the
    /// frames it sends to a file.
    DumpData(DumpDataArgs),

    /// Show, generate or locate config.toml.
    Config(ConfigArgs),
}

/// `st status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Print the raw status document as JSON.
    #[arg(long)]
    pub json: bool,
}

/// `st ls`.
#[derive(Debug, Args)]
pub struct LsArgs {
    /// Print the workspace document as JSON instead of a tree.
    #[arg(long)]
    pub json: bool,
}

/// `st probe`.
#[derive(Debug, Args)]
pub struct ProbeArgs {
    /// The surface to attach to; see `st ls` for the ids.
    #[arg(value_name = "SURFACE-ID")]
    pub surface: u32,

    /// Keep the connection open, apply Deltas and repaint on every change.
    #[arg(long, short = 'f')]
    pub follow: bool,

    /// Print one summary line per protocol message instead of the screen.
    #[arg(long, short = 'd')]
    pub dump: bool,

    /// Emit ANSI SGR from the style table instead of stripping styles.
    #[arg(long)]
    pub color: bool,

    /// Do not print the header line above the screen.
    #[arg(long)]
    pub no_header: bool,

    /// Stop after this many Deltas in --follow mode (0 = never).
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub max_deltas: u64,

    /// Attach mode: `active` streams rows, `passive` streams only metadata.
    #[arg(long, value_enum, default_value_t = ProbeMode::Active)]
    pub mode: ProbeMode,
}

/// The `Attach.mode` field (grilling Q44).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProbeMode {
    /// Rows, cursor, modes, title: everything.
    Active,
    /// Title, exit, bell and history_len only.
    Passive,
}

impl From<ProbeMode> for st_proto::AttachMode {
    fn from(mode: ProbeMode) -> Self {
        match mode {
            ProbeMode::Active => st_proto::AttachMode::Active,
            ProbeMode::Passive => st_proto::AttachMode::Passive,
        }
    }
}

/// `st kill-server`.
#[derive(Debug, Args)]
pub struct KillServerArgs {
    /// Shut down even with live surfaces, and fall back to SIGTERM on the pid
    /// in the lockfile when the control plane does not answer.
    #[arg(long)]
    pub force: bool,
}

/// `st dump-data`.
#[derive(Debug, Args)]
pub struct DumpDataArgs {
    /// The recording to decode; `-` reads stdin.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,

    /// Print one JSON object per frame, including the decoded body.
    #[arg(long)]
    pub json: bool,

    /// Stop after this many frames (0 = all).
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub limit: usize,
}

/// `st config`.
#[derive(Debug, Args)]
pub struct ConfigArgs {
    /// What to do with the config file.
    #[command(subcommand)]
    pub action: ConfigAction,
}

/// The `st config` actions.
#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Write a commented example config.toml.
    Init {
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
        /// Write here instead of the resolved config path; `-` writes stdout.
        #[arg(long, value_name = "PATH")]
        path: Option<PathBuf>,
    },
    /// Print the path config.toml is read from.
    Path,
    /// Print the effective configuration, defaults filled in.
    Show {
        /// Print JSON instead of TOML.
        #[arg(long)]
        json: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_tree_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_q46_subcommand_exists() {
        let cmd = Cli::command();
        let names: Vec<_> = cmd.get_subcommands().map(clap::Command::get_name).collect();
        for want in [
            "status",
            "ls",
            "probe",
            "kill-server",
            "dump-data",
            "config",
        ] {
            assert!(
                names.contains(&want),
                "missing subcommand `{want}` in {names:?}"
            );
        }
    }

    #[test]
    fn socket_is_a_global_flag() {
        let cli = Cli::try_parse_from(["st", "ls", "--socket", "/tmp/x.sock"]).unwrap();
        assert_eq!(cli.socket.unwrap().to_str().unwrap(), "/tmp/x.sock");

        let cli = Cli::try_parse_from(["st", "--socket", "/tmp/y.sock", "status"]).unwrap();
        assert_eq!(cli.socket.unwrap().to_str().unwrap(), "/tmp/y.sock");
    }

    #[test]
    fn probe_defaults_and_flags() {
        let cli = Cli::try_parse_from(["st", "probe", "9"]).unwrap();
        let Command::Probe(args) = cli.command else {
            panic!("expected probe")
        };
        assert_eq!(args.surface, 9);
        assert!(!args.follow && !args.dump && !args.color);
        assert_eq!(args.mode, ProbeMode::Active);

        let cli = Cli::try_parse_from(["st", "probe", "9", "--follow", "--color"]).unwrap();
        let Command::Probe(args) = cli.command else {
            panic!("expected probe")
        };
        assert!(args.follow && args.color);
    }

    #[test]
    fn a_non_numeric_surface_id_is_a_usage_error() {
        let err = Cli::try_parse_from(["st", "probe", "banana"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn the_help_documents_the_exit_codes() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("3  no server"));
        assert!(help.contains("5  not found"));
    }
}
