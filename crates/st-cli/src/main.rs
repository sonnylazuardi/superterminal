//! The `st` binary. Everything of substance lives in the `st_cli` library so
//! that it can be tested without spawning a process.

use std::io::Write;
use std::process::ExitCode as ProcessExit;

use clap::Parser;
use st_cli::cli::{Cli, Command, ConfigAction};
use st_cli::cmd;
use st_cli::exit::CliError;
use st_cli::exit::{ExitCode, Result};
use st_cli::render::RenderOptions;
use st_cli::transport::Transport;
use st_cli::transport::{
    resolve_lock_path, resolve_socket_path, resolve_tcp_addr, Connector, TcpConnector,
    UnixConnector,
};

fn main() -> ProcessExit {
    let cli = Cli::parse();
    init_logging(cli.log_level.as_deref(), cli.verbose);

    let mut stdout = std::io::stdout().lock();
    let result = dispatch(&cli, &mut stdout);
    // A closed pipe on flush is the `| head` case, not a failure.
    let _ = stdout.flush();

    match result {
        Ok(()) => ProcessExit::SUCCESS,
        Err(err) if err.exit == ExitCode::Success => ProcessExit::SUCCESS,
        Err(err) => {
            eprintln!("st: {err}");
            ProcessExit::from(err.exit as u8)
        }
    }
}

/// Either transport, chosen by `--tcp` / `$SUPERTERMINAL_TCP` / `--socket`.
#[derive(Debug, Clone)]
enum AnyConnector {
    Tcp(TcpConnector),
    Unix(UnixConnector),
}

impl AnyConnector {
    fn resolve(cli: &Cli) -> Self {
        match resolve_tcp_addr(cli.tcp) {
            Some(addr) => Self::Tcp(TcpConnector::new(addr)),
            None => Self::Unix(UnixConnector::new(resolve_socket_path(
                cli.socket.as_deref(),
            ))),
        }
    }
}

impl Connector for AnyConnector {
    fn connect(&self) -> std::result::Result<Box<dyn Transport>, CliError> {
        match self {
            Self::Tcp(c) => c.connect(),
            Self::Unix(c) => c.connect(),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Tcp(c) => c.describe(),
            Self::Unix(c) => c.describe(),
        }
    }
}

fn dispatch(cli: &Cli, out: &mut dyn Write) -> Result<()> {
    let connector = AnyConnector::resolve(cli);

    match &cli.command {
        Command::Status(args) => cmd::status::run(&connector, args.json, out),
        Command::Ls(args) => cmd::ls::run(&connector, args.json, out),
        Command::Probe(args) => cmd::probe::run(
            &connector,
            cmd::probe::ProbeConfig {
                surface: st_proto::SurfaceId(args.surface),
                follow: args.follow,
                dump: args.dump,
                render: RenderOptions {
                    color: args.color,
                    trim_trailing_blanks: true,
                },
                header: !args.no_header,
                max_deltas: args.max_deltas,
                mode: args.mode.into(),
            },
            out,
        ),
        Command::KillServer(args) => cmd::kill_server::run(
            &connector,
            &resolve_lock_path(cli.socket.as_deref()),
            args.force,
            out,
        ),
        Command::DumpData(args) => cmd::dump_data::run(&args.file, args.json, args.limit, out),
        Command::Config(args) => match &args.action {
            ConfigAction::Init { force, path } => cmd::config::init(path.as_deref(), *force, out),
            ConfigAction::Path => cmd::config::path(out),
            ConfigAction::Show { json } => cmd::config::show(*json, out),
        },
    }
}

/// `--log-level` beats `$SUPERTERMINAL_LOG`; both default to `warn` so that
/// `st ls` output stays machine-readable.
fn init_logging(level: Option<&str>, verbose: bool) {
    use tracing_subscriber::filter::EnvFilter;

    let filter = match (level, verbose) {
        (Some(level), _) => EnvFilter::new(level),
        (None, true) => EnvFilter::new("debug"),
        (None, false) => {
            EnvFilter::try_from_env("SUPERTERMINAL_LOG").unwrap_or_else(|_| EnvFilter::new("warn"))
        }
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}
