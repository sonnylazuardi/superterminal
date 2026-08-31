//! The emit loop — `03-server.md` §6, `02-protocol.md` §6.
//!
//! One tokio task per daemon (not per Surface) ticks at 120 Hz, walks every
//! Surface's [`Publisher`](st_core::Publisher) and turns the
//! [`Emission`](st_core::Emission)s it produces into `Snapshot` / `Delta` /
//! `Bell` frames on the right connections. `st-core` already owns the hard
//! parts — coalescing, the leading-edge 8.33 ms gap, the four-Delta ack window
//! and the 3 s forced Snapshot — so this module is the I/O half only:
//!
//! * reap exited children and fan out `SurfaceExited` (Q22);
//! * apply the Q44 Passive rule, which never sends rows;
//! * enforce `MAX_FRAME` (16 MiB, Q50), splitting `History` pages that do not
//!   fit;
//! * disconnect clients that have not acknowledged anything for 30 s;
//! * sample cwd, title and the foreground process group and report them to the
//!   Workspace actor so they land in `ev.workspace` (Q48).
//!
//! Trailing-blank trimming and the per-row `wrapped` flag (Q41) are done by
//! `st-core`'s row packer, so nothing here touches cell content.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use st_core::pty::Pty;
use st_core::publisher::ClientId;
use st_proto::{AbsLine, AttachMode, DataMsg, History, SurfaceId};

use crate::supervisor::{SurfaceSlot, SurfaceSupervisor, Upcall};
use crate::workspace::SurfaceEvent;

/// The 120 Hz emit interval (`03-server.md` §6).
pub const TICK: Duration = Duration::from_nanos(8_333_333);

/// Runs the emit loop until the supervisor is dropped.
pub async fn run(supervisor: Weak<SurfaceSupervisor>) {
    let slow_every = {
        let Some(sup) = supervisor.upgrade() else {
            return;
        };
        let ticks = sup.config().sample_interval.as_nanos() / TICK.as_nanos().max(1);
        u64::try_from(ticks).unwrap_or(120).max(1)
    };

    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ticks: u64 = 0;
    loop {
        interval.tick().await;
        let Some(sup) = supervisor.upgrade() else {
            return;
        };
        ticks = ticks.wrapping_add(1);
        tick(&sup, Instant::now(), ticks % slow_every == 0);
    }
}

/// One pass over every Surface. Synchronous, so it is directly unit-testable.
///
/// `slow` marks the once-a-second pass that reaps children that died without
/// closing the PTY, probes the cwd and drops silent clients.
pub fn tick(supervisor: &SurfaceSupervisor, now: Instant, slow: bool) {
    for slot in supervisor.surfaces() {
        reap(supervisor, &slot, slow);
        flush(supervisor, &slot, now);
        report_title(supervisor, &slot);
        if slow {
            sample(supervisor, &slot);
            disconnect_silent(supervisor, &slot, now);
        }
    }
}

// ------------------------------------------------------------------ exit

fn reap(supervisor: &SurfaceSupervisor, slot: &Arc<SurfaceSlot>, slow: bool) {
    if slot.exit_reported() || !(slow || slot.exit_hint()) {
        return;
    }
    let exited = slot.lock().poll_exit();
    let Some(exited) = exited else {
        return;
    };
    slot.mark_exit_reported();
    tracing::info!(surface = %slot.id(), status = ?exited.status, "surface exited");
    if let Some(metrics) = supervisor.metrics() {
        metrics.surfaces_exited.inc();
    }

    // `st_proto::ExitStatus` has no room for a signal *name*; the Surface's
    // own status does, and the Workspace document shows it (§3).
    let (clients, signal) = {
        let surface = slot.lock();
        let signal = match surface.status() {
            st_core::SurfaceStatus::Exited(status) => status.signal_name.clone(),
            st_core::SurfaceStatus::Running { .. } => None,
        };
        (
            surface.publisher().clients().collect::<Vec<ClientId>>(),
            signal,
        )
    };
    for client in clients {
        supervisor.send(client, &DataMsg::SurfaceExited(exited));
    }
    supervisor.notify(Upcall::Surface(SurfaceEvent::Exited {
        surface: slot.id(),
        code: exited.status.code,
        signal,
    }));
}

// ----------------------------------------------------------------- frames

fn flush(supervisor: &SurfaceSupervisor, slot: &Arc<SurfaceSlot>, now: Instant) {
    let (frames, modes) = {
        let mut surface = slot.lock();
        if !surface.should_flush(now) {
            return;
        }
        let frames = surface.flush(now);
        let modes: BTreeMap<ClientId, AttachMode> = frames
            .iter()
            .map(|frame| {
                let mode = surface
                    .publisher()
                    .subscription(frame.client)
                    .map_or(AttachMode::Active, |sub| sub.mode());
                (frame.client, mode)
            })
            .collect();
        (frames, modes)
    };

    for frame in frames {
        let mut msg = frame.msg;
        if modes.get(&frame.client) == Some(&AttachMode::Passive) {
            strip_rows(&mut msg);
        }
        send(supervisor, frame.client, msg);
    }
}

/// Grilling Q44: a Passive subscription is told about the title, the exit, the
/// bell and `history_len`, and never about rows.
///
/// The `Publisher` already withholds dirty rows from a Passive Delta; only the
/// Snapshot that every Attach begins with has to be emptied here.
fn strip_rows(msg: &mut DataMsg) {
    match msg {
        DataMsg::Snapshot(snapshot) => {
            snapshot.grid.clear();
            snapshot.styles.clear();
        }
        DataMsg::Delta(delta) => {
            delta.rows.clear();
            delta.new_styles.clear();
        }
        _ => {}
    }
}

/// Encodes one message and queues it, enforcing `MAX_FRAME` (Q50).
///
/// A `History` page that does not fit is split in half and retried; anything
/// else that does not fit is a bug in the size accounting, so the client is
/// told rather than silently starved.
pub fn send(supervisor: &SurfaceSupervisor, client: ClientId, msg: DataMsg) -> bool {
    let mut wire = Vec::new();
    match msg.encode_to(&mut wire) {
        Ok(()) => {
            supervisor.count_frame(msg.msg_type());
            supervisor.send_bytes(client, wire)
        }
        Err(err) => match msg {
            DataMsg::History(page) => {
                let mut pages = Vec::new();
                split_history(*page, &mut pages);
                pages
                    .into_iter()
                    .all(|wire| supervisor.send_bytes(client, wire))
            }
            other => {
                tracing::error!(
                    %client,
                    msg_type = other.msg_type(),
                    %err,
                    "a frame does not fit in MAX_FRAME"
                );
                supervisor.send(
                    client,
                    &DataMsg::DataError(st_proto::DataError {
                        surface_id: surface_of(&other),
                        code: st_proto::DATA_ERR_BAD_REQUEST,
                        message: format!("frame does not fit in {} bytes", st_proto::MAX_FRAME),
                    }),
                )
            }
        },
    }
}

/// Encodes a `History` page, halving it until every part fits in a frame.
fn split_history(page: History, out: &mut Vec<Vec<u8>>) {
    let msg = DataMsg::History(Box::new(page));
    let mut wire = Vec::new();
    if msg.encode_to(&mut wire).is_ok() {
        out.push(wire);
        return;
    }
    let DataMsg::History(page) = msg else {
        unreachable!("just built a History")
    };
    let mut page = *page;
    if page.rows.len() <= 1 {
        tracing::error!(
            surface = %page.surface_id,
            from = %page.from_line,
            "a single history row exceeds MAX_FRAME; dropping it"
        );
        return;
    }
    let mid = page.rows.len() / 2;
    let tail_rows = page.rows.split_off(mid);
    let tail = History {
        surface_id: page.surface_id,
        from_line: AbsLine::new(page.from_line.get() + u64::try_from(mid).unwrap_or(0)),
        history_base: page.history_base,
        rows: tail_rows,
    };
    split_history(page, out);
    split_history(tail, out);
}

fn surface_of(msg: &DataMsg) -> Option<SurfaceId> {
    match msg {
        DataMsg::Snapshot(m) => Some(m.surface_id),
        DataMsg::Delta(m) => Some(m.surface_id),
        DataMsg::History(m) => Some(m.surface_id),
        DataMsg::SurfaceExited(m) => Some(m.surface_id),
        DataMsg::Bell(m) => Some(m.surface_id),
        DataMsg::Detached(m) => Some(m.surface_id),
        _ => None,
    }
}

// ------------------------------------------------------------ housekeeping

fn report_title(supervisor: &SurfaceSupervisor, slot: &Arc<SurfaceSlot>) {
    let changed = {
        let surface = slot.lock();
        slot.take_title_change(surface.title())
    };
    if let Some(title) = changed {
        supervisor.notify(Upcall::Surface(SurfaceEvent::Title {
            surface: slot.id(),
            title,
        }));
    }
}

fn sample(supervisor: &SurfaceSupervisor, slot: &Arc<SurfaceSlot>) {
    let (cwd, busy) = {
        let mut surface = slot.lock();
        surface.probe_cwd();
        let cwd = surface.take_cwd_change();
        let pid = surface.pty().and_then(Pty::pid);
        let foreground = surface.pty().and_then(Pty::foreground_pgid);
        let busy = match (pid, foreground) {
            (Some(pid), Some(fg)) => pid != fg,
            _ => false,
        };
        (cwd, busy)
    };
    if let Some(cwd) = cwd {
        supervisor.notify(Upcall::Surface(SurfaceEvent::Cwd {
            surface: slot.id(),
            cwd: cwd.to_string_lossy().into_owned(),
        }));
    }
    if let Some(present) = slot.take_busy_change(busy) {
        supervisor.notify(Upcall::Surface(SurfaceEvent::ForegroundChild {
            surface: slot.id(),
            present,
        }));
    }
}

/// Closes connections that have not acknowledged anything for
/// [`PublisherConfig::disconnect_after`](st_core::PublisherConfig) (§6).
fn disconnect_silent(supervisor: &SurfaceSupervisor, slot: &Arc<SurfaceSlot>, now: Instant) {
    let silent = slot.lock().publisher().silent_clients(now);
    for client in silent {
        tracing::warn!(%client, surface = %slot.id(), "no Ack for 30 s; closing the connection");
        slot.lock().detach(client);
        supervisor.close_client(client);
    }
}
