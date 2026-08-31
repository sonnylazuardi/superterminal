//! `st probe` — the M1 acceptance tool.
//!
//! Attaches to a Surface on the DATA plane and prints its screen as text. No
//! GPU, no GPUI, no window: if `st probe 9` shows a shell prompt, the server's
//! PTY, VT engine, style table, delta pipeline and framing all work.
//!
//! The sequence is the one in `02-protocol.md`: magic → `Hello` → `HelloAck` →
//! `Attach{want_snapshot: true, known_seq: 0}` → `Snapshot`. With `--follow`
//! the loop continues: apply each `Delta` (§6.2), `Ack` it (§6.5), repaint. A
//! `since_seq` mismatch (§6.3) triggers a fresh `Attach` with
//! `want_snapshot: true` rather than showing a corrupt screen.

use std::io::Write;

use st_proto::{AttachMode, DataMsg, DetachReason, ExitStatus, Seq, SurfaceId};

use crate::cmd::dump_data::summarize;
use crate::dataplane::DataClient;
use crate::exit::{CliError, ExitCode, Result};
use crate::render::{render_grid, RenderOptions};
use crate::replica::Replica;
use crate::transport::Connector;

/// Everything `st probe` needs to know.
#[derive(Debug, Clone, Copy)]
pub struct ProbeConfig {
    /// The Surface to attach to.
    pub surface: SurfaceId,
    /// Keep applying Deltas and repainting.
    pub follow: bool,
    /// Print protocol message summaries instead of the screen.
    pub dump: bool,
    /// How to render the grid.
    pub render: RenderOptions,
    /// Print the header line above each paint.
    pub header: bool,
    /// Stop after this many Deltas in follow mode; `0` never stops.
    pub max_deltas: u64,
    /// `Attach.mode` (grilling Q44).
    pub mode: AttachMode,
}

/// Runs the command.
pub fn run(connector: &dyn Connector, cfg: ProbeConfig, out: &mut dyn Write) -> Result<()> {
    let mut client = DataClient::connect(connector)?;
    client.attach(cfg.surface, cfg.mode)?;

    let mut replica: Option<Replica> = None;
    let mut deltas = 0u64;
    let mut index = 0usize;

    loop {
        let Some(msg) = client.recv()? else {
            return if replica.is_some() || cfg.dump {
                Ok(())
            } else {
                Err(CliError::protocol(format!(
                    "server closed the connection without sending a Snapshot for surface {}",
                    cfg.surface
                )))
            };
        };

        if cfg.dump {
            writeln!(out, "{}", summarize(index, &msg)).map_err(write_error)?;
            index += 1;
        }

        match msg {
            DataMsg::Snapshot(snap) => {
                if snap.surface_id != cfg.surface {
                    tracing::debug!(got = %snap.surface_id, "snapshot for another surface");
                    continue;
                }
                let new = Replica::from_snapshot(&snap);
                client.ack(cfg.surface, new.seq)?;
                let exited = new.exited;
                if !cfg.dump {
                    paint(out, &new, cfg)?;
                }
                replica = Some(new);
                if let Some(status) = exited {
                    return finish(out, cfg, status);
                }
                if !cfg.follow {
                    return Ok(());
                }
            }
            DataMsg::Delta(delta) => {
                if delta.surface_id != cfg.surface {
                    continue;
                }
                let Some(current) = replica.as_mut() else {
                    // §6.3: discard deltas until the Snapshot arrives.
                    continue;
                };
                if let Err(gap) = current.apply_delta(&delta) {
                    tracing::warn!(%gap, "re-attaching after a sequence gap");
                    replica = None;
                    client.attach(cfg.surface, cfg.mode)?;
                    continue;
                }
                let seq = current.seq;
                client.ack(cfg.surface, seq)?;
                if !cfg.dump {
                    paint(out, current, cfg)?;
                }
                deltas += 1;
                if cfg.max_deltas != 0 && deltas >= cfg.max_deltas {
                    return Ok(());
                }
            }
            DataMsg::SurfaceExited(exited) if exited.surface_id == cfg.surface => {
                if let Some(current) = replica.as_mut() {
                    current.seq = exited.seq;
                    current.exited = Some(exited.status);
                }
                return finish(out, cfg, exited.status);
            }
            DataMsg::Detached(detached) if detached.surface_id == cfg.surface => {
                return match detached.reason {
                    DetachReason::Requested => Ok(()),
                    DetachReason::SurfaceDestroyed => Err(CliError::new(
                        ExitCode::NotFound,
                        format!("surface {} was destroyed", cfg.surface),
                    )),
                    DetachReason::ServerShutdown => {
                        Err(CliError::no_server("the server is shutting down"))
                    }
                };
            }
            DataMsg::DataError(err) => {
                return Err(CliError::new(
                    if err.code == st_proto::DATA_ERR_NOT_ATTACHED {
                        ExitCode::NotFound
                    } else {
                        ExitCode::Refused
                    },
                    format!("server error 0x{:04X}: {}", err.code, err.message),
                ));
            }
            DataMsg::Bell(_) | DataMsg::History(_) => {}
            other => {
                tracing::debug!(msg_type = other.msg_type(), "ignoring message");
            }
        }
    }
}

/// The header plus the grid.
fn paint(out: &mut dyn Write, replica: &Replica, cfg: ProbeConfig) -> Result<()> {
    if cfg.header {
        writeln!(out, "{}", header(replica)).map_err(write_error)?;
    }
    out.write_all(render_grid(replica, cfg.render).as_bytes())
        .map_err(write_error)
}

/// The one-line banner above the screen.
#[must_use]
pub fn header(replica: &Replica) -> String {
    format!(
        "surface {} seq {} {}x{} history {} title {:?}{}",
        replica.surface_id,
        replica.seq,
        replica.cols,
        replica.rows,
        replica.history_len,
        replica.title,
        if replica.modes.contains(st_proto::Modes::ALT_SCREEN) {
            " [alt]"
        } else {
            ""
        }
    )
}

/// The exit notice, and the non-zero code that goes with a failed process.
fn finish(out: &mut dyn Write, cfg: ProbeConfig, status: ExitStatus) -> Result<()> {
    if !cfg.dump {
        writeln!(out, "{}", describe_exit(status)).map_err(write_error)?;
    }
    Ok(())
}

/// `[surface exited: code 0]`, `[surface exited: signal 9]`, or unknown.
#[must_use]
pub fn describe_exit(status: ExitStatus) -> String {
    match (status.code, status.signal) {
        (Some(code), _) => format!("[surface exited: code {code}]"),
        (None, Some(signal)) => format!("[surface exited: signal {signal}]"),
        (None, None) => "[surface exited]".to_string(),
    }
}

fn write_error(err: std::io::Error) -> CliError {
    if err.kind() == std::io::ErrorKind::BrokenPipe {
        // `st probe … | head` is a normal way to use this.
        return CliError::new(ExitCode::Success, "output pipe closed");
    }
    CliError::failure(format!("cannot write output: {err}"))
}

/// The `Seq` a fresh probe claims to know: nothing (§4.2).
pub const UNKNOWN_SEQ: Seq = Seq(0);

#[cfg(test)]
mod tests {
    use super::*;
    use st_proto::{AbsLine, Cursor, Modes, Row, Snapshot, Style, ViewState};

    fn replica(title: &str, alt: bool) -> Replica {
        Replica::from_snapshot(&Snapshot {
            surface_id: SurfaceId(9),
            seq: Seq(77),
            cols: 80,
            rows: 24,
            styles: vec![Style::DEFAULT],
            grid: vec![Row::new(); 24],
            cursor: Cursor::default(),
            modes: if alt {
                Modes::ALT_SCREEN
            } else {
                Modes::LINE_WRAP
            },
            title: title.into(),
            history_base: AbsLine(0),
            history_len: 512,
            view_state: ViewState::default(),
            exited: None,
        })
    }

    #[test]
    fn the_header_names_everything_a_reader_needs() {
        assert_eq!(
            header(&replica("zsh", false)),
            "surface 9 seq 77 80x24 history 512 title \"zsh\""
        );
    }

    #[test]
    fn the_header_flags_the_alternate_screen() {
        assert!(header(&replica("vim", true)).ends_with(" [alt]"));
    }

    #[test]
    fn exit_descriptions_cover_every_shape() {
        assert_eq!(
            describe_exit(ExitStatus {
                code: Some(0),
                signal: None
            }),
            "[surface exited: code 0]"
        );
        assert_eq!(
            describe_exit(ExitStatus {
                code: None,
                signal: Some(9)
            }),
            "[surface exited: signal 9]"
        );
        assert_eq!(
            describe_exit(ExitStatus {
                code: None,
                signal: None
            }),
            "[surface exited]"
        );
    }

    #[test]
    fn a_broken_pipe_is_not_a_failure() {
        let err = write_error(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        assert_eq!(err.exit, ExitCode::Success);
        let err = write_error(std::io::Error::from(std::io::ErrorKind::StorageFull));
        assert_eq!(err.exit, ExitCode::Failure);
    }
}
