//! Delta production and fan-out (`docs/plan/03-server.md` §6).
//!
//! A [`Publisher`] is a **pure state machine over an injected clock**: it owns
//! no channels, no tasks and no frames. It answers one question — *for this
//! Surface, at this instant, what must each attached Client be sent?* — and the
//! Server (or [`crate::surface::Surface`]) materialises the answer. That is why
//! every rule below is unit-testable without tokio.
//!
//! The rules it implements:
//!
//! * **Coalescing.** Damage is ORed into every subscription's pending set. Row
//!   *content* is never captured here; it is read from the engine when the
//!   frame is built, so a Delta always carries the latest state and the memory
//!   held for a client that is far behind is one bitset plus a few scalars
//!   (Q27).
//! * **120 Hz.** A minimum gap of [`PublisherConfig::min_interval`] between
//!   flushes, leading-edge: the first change after an idle period flushes
//!   immediately, the rest wait for the next tick.
//! * **Ack window.** At most [`PublisherConfig::ack_window`] Deltas may be in
//!   flight per subscription ([`st_proto::MAX_UNACKED_DELTAS`] = 4). A blocked
//!   subscription is skipped, never buffered.
//! * **Slow-client policy.** Blocked for
//!   [`PublisherConfig::slow_client_snapshot_after`] (3 s) ⇒ force a Snapshot,
//!   which is cheaper than replaying and guarantees convergence. No Ack at all
//!   for [`PublisherConfig::disconnect_after`] (30 s) ⇒ the Server drops the
//!   connection.
//! * **Passive attach (Q44).** A Passive subscription never receives rows,
//!   only title, bell, exit and `history_len`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use st_proto::{AttachMode, Seq, MAX_UNACKED_DELTAS};

use crate::vt::{Damage, DirtySet};

/// Identifies one Client *connection*. Never persisted, never on the wire —
/// `st-proto` deliberately has no such id (invariant I8), so it lives here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ClientId(pub u64);

impl ClientId {
    /// Wraps a raw id.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw id.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// Tunables from `[server]` (`03-server.md` §6, OQ10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublisherConfig {
    /// Minimum gap between flushes; 120 Hz = 8.333 ms.
    pub min_interval: Duration,
    /// Deltas allowed in flight per subscription before it blocks.
    pub ack_window: u32,
    /// How long a subscription may stay window-blocked before it is given a
    /// Snapshot instead.
    pub slow_client_snapshot_after: Duration,
    /// How long a subscription may go without any Ack before the Server closes
    /// its connection.
    pub disconnect_after: Duration,
}

impl Default for PublisherConfig {
    fn default() -> Self {
        Self {
            min_interval: Duration::from_nanos(8_333_333),
            ack_window: MAX_UNACKED_DELTAS,
            slow_client_snapshot_after: Duration::from_secs(3),
            disconnect_after: Duration::from_secs(30),
        }
    }
}

/// What has changed since a subscription's last frame.
///
/// Everything here is a *flag*, not content: the frame builder re-reads the
/// engine, which is what makes coalescing bounded (Q27).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coalesced {
    /// Rows whose content changed.
    pub dirty: DirtySet,
    /// The cursor moved or changed shape.
    pub cursor: bool,
    /// A mode bit flipped.
    pub modes: bool,
    /// The title changed (OSC 0/2).
    pub title: bool,
    /// The program rang the bell at least once; ORed, never counted.
    pub bell: bool,
    /// `history_base` or `history_len` changed.
    pub history: bool,
    /// The grid was resized to `(cols, rows)`.
    pub resized: Option<(u16, u16)>,
}

impl Coalesced {
    /// An empty accumulator sized for `rows` visible rows.
    #[must_use]
    pub fn new(rows: usize) -> Self {
        Self {
            dirty: DirtySet::new(rows),
            ..Self::default()
        }
    }

    /// `true` when there is nothing to send.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dirty.is_empty()
            && !self.cursor
            && !self.modes
            && !self.title
            && !self.bell
            && !self.history
            && self.resized.is_none()
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        self.dirty.clear();
        self.cursor = false;
        self.modes = false;
        self.title = false;
        self.bell = false;
        self.history = false;
        self.resized = None;
    }
}

/// One Client's view of one Surface (`03-server.md` §6).
#[derive(Debug, Clone)]
pub struct Subscription {
    mode: AttachMode,
    last_sent_seq: Seq,
    last_acked_seq: Seq,
    pending: Coalesced,
    needs_snapshot: bool,
    stalled_since: Option<Instant>,
    last_ack_at: Instant,
}

impl Subscription {
    /// Active or Passive (grilling Q44).
    #[must_use]
    pub fn mode(&self) -> AttachMode {
        self.mode
    }

    /// The sequence number of the last frame handed to this subscription.
    #[must_use]
    pub fn last_sent_seq(&self) -> Seq {
        self.last_sent_seq
    }

    /// The highest sequence number this Client has acknowledged.
    #[must_use]
    pub fn last_acked_seq(&self) -> Seq {
        self.last_acked_seq
    }

    /// What has accumulated since the last frame.
    #[must_use]
    pub fn pending(&self) -> &Coalesced {
        &self.pending
    }

    /// `true` when the next frame must be a full Snapshot.
    #[must_use]
    pub fn needs_snapshot(&self) -> bool {
        self.needs_snapshot
    }

    /// `true` when the ack window is full, so no Delta may be sent.
    #[must_use]
    pub fn is_window_blocked(&self, ack_window: u32) -> bool {
        self.last_sent_seq
            .get()
            .saturating_sub(self.last_acked_seq.get())
            >= u64::from(ack_window)
    }

    /// Since when this subscription has been window-blocked.
    #[must_use]
    pub fn stalled_since(&self) -> Option<Instant> {
        self.stalled_since
    }
}

/// What one Client must be sent in this flush.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emission {
    /// Who it is for.
    pub client: ClientId,
    /// Active or Passive.
    pub mode: AttachMode,
    /// The frame to build.
    pub kind: EmissionKind,
    /// A standalone `Bell` must accompany the frame (it is an event, not
    /// state, so it is outside the sequence — grilling Q38).
    pub bell: bool,
}

/// The shape of the frame a Client is owed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmissionKind {
    /// A full [`st_proto::Snapshot`].
    Snapshot,
    /// A [`st_proto::Delta`] carrying exactly these rows.
    Delta {
        /// Rows to include; always empty for a Passive subscription.
        dirty: DirtySet,
        /// Include the title (it changed).
        title: bool,
        /// `Some` when the grid was resized in this frame.
        resized: Option<(u16, u16)>,
    },
    /// Nothing but the bell: no state changed for this subscription.
    BellOnly,
}

/// Per-Surface fan-out (`03-server.md` §6).
#[derive(Debug)]
pub struct Publisher {
    config: PublisherConfig,
    subs: HashMap<ClientId, Subscription>,
    rows: usize,
    last_flush: Option<Instant>,
}

impl Publisher {
    /// A Publisher for a Surface `rows` rows tall.
    #[must_use]
    pub fn new(config: PublisherConfig, rows: usize) -> Self {
        Self {
            config,
            subs: HashMap::new(),
            rows,
            last_flush: None,
        }
    }

    /// The tunables in force.
    #[must_use]
    pub fn config(&self) -> &PublisherConfig {
        &self.config
    }

    /// Number of attached Clients.
    #[must_use]
    pub fn len(&self) -> usize {
        self.subs.len()
    }

    /// `true` when nobody is attached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.subs.is_empty()
    }

    /// Looks a subscription up.
    #[must_use]
    pub fn subscription(&self, client: ClientId) -> Option<&Subscription> {
        self.subs.get(&client)
    }

    /// Attached Clients, in no particular order.
    pub fn clients(&self) -> impl Iterator<Item = ClientId> + '_ {
        self.subs.keys().copied()
    }

    /// Subscribes `client`. The first frame is always a Snapshot (§6, Attach).
    ///
    /// Returns `false` when the Client is already attached: a second Attach to
    /// the same Surface from one connection is rejected (§6).
    pub fn attach(&mut self, client: ClientId, mode: AttachMode, now: Instant) -> bool {
        if self.subs.contains_key(&client) {
            return false;
        }
        let mut pending = Coalesced::new(self.rows);
        pending.dirty.set_all();
        pending.cursor = true;
        pending.modes = true;
        pending.title = true;
        pending.history = true;
        self.subs.insert(
            client,
            Subscription {
                mode,
                last_sent_seq: Seq::ZERO,
                last_acked_seq: Seq::ZERO,
                pending,
                needs_snapshot: true,
                stalled_since: None,
                last_ack_at: now,
            },
        );
        true
    }

    /// Unsubscribes `client`; returns `true` if it was attached.
    pub fn detach(&mut self, client: ClientId) -> bool {
        self.subs.remove(&client).is_some()
    }

    /// Switches a subscription between Active and Passive (Q44).
    ///
    /// Becoming Active forces a Snapshot: the Client has no rows at all.
    pub fn set_mode(&mut self, client: ClientId, mode: AttachMode) {
        if let Some(sub) = self.subs.get_mut(&client) {
            if sub.mode != mode {
                sub.mode = mode;
                if mode == AttachMode::Active {
                    sub.needs_snapshot = true;
                    sub.pending.dirty.set_all();
                }
            }
        }
    }

    /// Records an Ack (§6.5).
    pub fn ack(&mut self, client: ClientId, seq: Seq, now: Instant) {
        if let Some(sub) = self.subs.get_mut(&client) {
            if seq > sub.last_acked_seq {
                sub.last_acked_seq = seq.min(sub.last_sent_seq);
            }
            sub.last_ack_at = now;
            if !sub.is_window_blocked(self.config.ack_window) {
                sub.stalled_since = None;
            }
        }
    }

    /// ORs engine damage into every subscription.
    pub fn record_damage(&mut self, damage: &Damage) {
        match damage {
            Damage::Full => self.for_each_pending(|p| p.dirty.set_all()),
            Damage::Rows(set) => {
                if set.is_empty() {
                    return;
                }
                self.for_each_pending(|p| p.dirty.union_with(set));
            }
        }
    }

    /// Marks the cursor as changed.
    pub fn record_cursor(&mut self) {
        self.for_each_pending(|p| p.cursor = true);
    }

    /// Marks the mode set as changed.
    pub fn record_modes(&mut self) {
        self.for_each_pending(|p| p.modes = true);
    }

    /// Marks the title as changed.
    pub fn record_title(&mut self) {
        self.for_each_pending(|p| p.title = true);
    }

    /// Records a bell; ORed, so a storm of bells is one flag.
    pub fn record_bell(&mut self) {
        self.for_each_pending(|p| p.bell = true);
    }

    /// Marks `history_base`/`history_len` as changed.
    pub fn record_history(&mut self) {
        self.for_each_pending(|p| p.history = true);
    }

    /// Records a resize: every row is dirty and the new size rides along.
    pub fn record_resize(&mut self, cols: u16, rows: u16) {
        self.rows = rows as usize;
        let size = (cols, rows);
        for sub in self.subs.values_mut() {
            sub.pending.dirty.resize(rows as usize);
            sub.pending.dirty.set_all();
            sub.pending.resized = Some(size);
            sub.pending.cursor = true;
            sub.pending.modes = true;
            sub.pending.history = true;
        }
    }

    /// Forces the next frame on every subscription to be a Snapshot.
    ///
    /// Used when the style table overflows and is reset (grilling Q45), and
    /// after a terminal reset.
    pub fn force_snapshot_all(&mut self) {
        for sub in self.subs.values_mut() {
            sub.needs_snapshot = true;
            sub.pending.dirty.set_all();
        }
    }

    /// `true` when at least one subscription has something to send.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.subs
            .values()
            .any(|s| s.needs_snapshot || !s.pending.is_empty())
    }

    /// `true` when the minimum inter-flush gap has elapsed.
    #[must_use]
    pub fn is_due(&self, now: Instant) -> bool {
        self.last_flush
            .is_none_or(|last| now.saturating_duration_since(last) >= self.config.min_interval)
    }

    /// `true` when [`Publisher::flush`] should be called right now.
    ///
    /// A pending Snapshot (a fresh Attach) bypasses the timer: §6 wants the
    /// first frame immediately.
    #[must_use]
    pub fn should_flush(&self, now: Instant) -> bool {
        if !self.has_pending() {
            return false;
        }
        self.is_due(now) || self.subs.values().any(|s| s.needs_snapshot)
    }

    /// When the next flush becomes due, or `None` when nothing is pending.
    ///
    /// The Server arms exactly one timer per Surface from this; there are no
    /// idle ticks.
    #[must_use]
    pub fn next_flush_at(&self) -> Option<Instant> {
        if !self.has_pending() {
            return None;
        }
        Some(match self.last_flush {
            Some(last) => last + self.config.min_interval,
            None => Instant::now(),
        })
    }

    /// Clients that have not acknowledged anything for
    /// [`PublisherConfig::disconnect_after`]; the Server closes them (§6).
    #[must_use]
    pub fn silent_clients(&self, now: Instant) -> Vec<ClientId> {
        self.subs
            .iter()
            .filter(|(_, s)| {
                s.last_sent_seq > s.last_acked_seq
                    && now.saturating_duration_since(s.last_ack_at) >= self.config.disconnect_after
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Decides what each Client is owed and marks it as sent.
    ///
    /// `seq` is the sequence number the frames of this round will carry: one
    /// flush describes one coalesced state, so every Client that receives
    /// something receives the same `seq`.
    ///
    /// The returned vector is sorted by [`ClientId`] so the output is
    /// deterministic for tests and logs.
    pub fn flush(&mut self, now: Instant, seq: Seq) -> Vec<Emission> {
        self.last_flush = Some(now);
        let ack_window = self.config.ack_window;
        let slow_after = self.config.slow_client_snapshot_after;

        let mut out = Vec::new();
        for (client, sub) in &mut self.subs {
            if !sub.needs_snapshot && sub.pending.is_empty() {
                continue;
            }
            let bell = sub.pending.bell;

            // A window-blocked subscription buffers nothing; after
            // `slow_after` it is converged with a Snapshot instead.
            if !sub.needs_snapshot && sub.is_window_blocked(ack_window) {
                let since = *sub.stalled_since.get_or_insert(now);
                if now.saturating_duration_since(since) < slow_after {
                    if bell {
                        // The bell is outside the sequence, so it is never
                        // held back by the window.
                        sub.pending.bell = false;
                        out.push(Emission {
                            client: *client,
                            mode: sub.mode,
                            kind: EmissionKind::BellOnly,
                            bell: true,
                        });
                    }
                    continue;
                }
                sub.needs_snapshot = true;
            }

            if sub.needs_snapshot {
                sub.needs_snapshot = false;
                sub.pending.clear();
                sub.last_sent_seq = seq;
                // One frame outstanding again, so the window reopens.
                sub.last_acked_seq = Seq::new(seq.get().saturating_sub(1));
                sub.stalled_since = None;
                sub.last_ack_at = now;
                out.push(Emission {
                    client: *client,
                    mode: sub.mode,
                    kind: EmissionKind::Snapshot,
                    bell,
                });
                continue;
            }

            let dirty = if sub.mode == AttachMode::Active {
                std::mem::replace(&mut sub.pending.dirty, DirtySet::new(self.rows))
            } else {
                DirtySet::new(0)
            };
            let title = sub.pending.title;
            let resized = sub.pending.resized;
            sub.pending.clear();
            sub.last_sent_seq = seq;
            sub.stalled_since = None;
            out.push(Emission {
                client: *client,
                mode: sub.mode,
                kind: EmissionKind::Delta {
                    dirty,
                    title,
                    resized,
                },
                bell,
            });
        }
        out.sort_by_key(|e| e.client);
        out
    }

    fn for_each_pending(&mut self, mut f: impl FnMut(&mut Coalesced)) {
        for sub in self.subs.values_mut() {
            f(&mut sub.pending);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C1: ClientId = ClientId(1);
    const C2: ClientId = ClientId(2);

    fn publisher(rows: usize) -> (Publisher, Instant) {
        (
            Publisher::new(PublisherConfig::default(), rows),
            Instant::now(),
        )
    }

    fn dirty(rows: usize, lines: &[usize]) -> Damage {
        let mut set = DirtySet::new(rows);
        for &l in lines {
            set.set(l);
        }
        Damage::Rows(set)
    }

    /// Drains the initial Snapshot an Attach always produces.
    fn settle(pub_: &mut Publisher, client: ClientId, now: Instant, seq: u64) -> Instant {
        let out = pub_.flush(now, Seq::new(seq));
        assert!(matches!(out[0].kind, EmissionKind::Snapshot));
        pub_.ack(client, Seq::new(seq), now);
        now + Duration::from_millis(20)
    }

    #[test]
    fn attach_yields_a_snapshot_first() {
        let (mut p, t0) = publisher(24);
        assert!(p.attach(C1, AttachMode::Active, t0));
        assert!(
            !p.attach(C1, AttachMode::Active, t0),
            "double attach refused"
        );
        assert!(p.should_flush(t0), "the first frame is immediate");

        let out = p.flush(t0, Seq::new(1));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].client, C1);
        assert_eq!(out[0].kind, EmissionKind::Snapshot);
        assert!(!p.has_pending());
    }

    #[test]
    fn coalescing_merges_dirty_rows_per_client() {
        let (mut p, t0) = publisher(24);
        p.attach(C1, AttachMode::Active, t0);
        let t1 = settle(&mut p, C1, t0, 1);

        p.record_damage(&dirty(24, &[1, 2]));
        p.record_damage(&dirty(24, &[2, 7]));
        p.record_cursor();
        p.record_bell();
        p.record_bell();

        let out = p.flush(t1, Seq::new(2));
        assert_eq!(out.len(), 1);
        assert!(out[0].bell, "bells are ORed into one flag");
        match &out[0].kind {
            EmissionKind::Delta { dirty, .. } => {
                assert_eq!(dirty.iter().collect::<Vec<_>>(), vec![1, 2, 7]);
            }
            other => panic!("expected a Delta, got {other:?}"),
        }
        assert!(!p.has_pending(), "a flush clears the pending set");
    }

    #[test]
    fn a_late_attacher_gets_a_snapshot_while_others_get_deltas() {
        let (mut p, t0) = publisher(10);
        p.attach(C1, AttachMode::Active, t0);
        let t1 = settle(&mut p, C1, t0, 1);

        p.attach(C2, AttachMode::Active, t1);
        p.record_damage(&dirty(10, &[3]));
        let out = p.flush(t1, Seq::new(2));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].client, C1);
        assert!(matches!(out[0].kind, EmissionKind::Delta { .. }));
        assert_eq!(out[1].client, C2);
        assert_eq!(out[1].kind, EmissionKind::Snapshot);
    }

    #[test]
    fn passive_subscriptions_never_carry_rows() {
        let (mut p, t0) = publisher(10);
        p.attach(C1, AttachMode::Passive, t0);
        let t1 = settle(&mut p, C1, t0, 1);

        p.record_damage(&dirty(10, &[0, 1, 2]));
        p.record_title();
        let out = p.flush(t1, Seq::new(2));
        match &out[0].kind {
            EmissionKind::Delta { dirty, title, .. } => {
                assert!(dirty.is_empty(), "Q44: a Passive attach gets no rows");
                assert!(title);
            }
            other => panic!("expected a Delta, got {other:?}"),
        }

        // Going Active re-syncs from scratch.
        p.set_mode(C1, AttachMode::Active);
        let out = p.flush(t1 + Duration::from_millis(20), Seq::new(3));
        assert_eq!(out[0].kind, EmissionKind::Snapshot);
    }

    #[test]
    fn the_ack_window_blocks_after_four_unacked_deltas() {
        let (mut p, t0) = publisher(10);
        p.attach(C1, AttachMode::Active, t0);
        let mut now = settle(&mut p, C1, t0, 1);

        // Four Deltas go out unacknowledged.
        for seq in 2..=5 {
            p.record_damage(&dirty(10, &[0]));
            let out = p.flush(now, Seq::new(seq));
            assert_eq!(out.len(), 1, "seq {seq} should still fit in the window");
            now += Duration::from_millis(10);
        }
        let sub = p.subscription(C1).unwrap();
        assert!(sub.is_window_blocked(4));

        // The fifth is withheld and keeps coalescing.
        p.record_damage(&dirty(10, &[4]));
        assert!(p.flush(now, Seq::new(6)).is_empty(), "window is full");
        now += Duration::from_millis(10);
        p.record_damage(&dirty(10, &[5]));
        assert!(p.flush(now, Seq::new(6)).is_empty());
        assert!(p.has_pending(), "withheld damage is not lost");

        // An Ack reopens the window and the union is delivered at once.
        p.ack(C1, Seq::new(5), now);
        now += Duration::from_millis(10);
        let out = p.flush(now, Seq::new(6));
        match &out[0].kind {
            EmissionKind::Delta { dirty, .. } => {
                assert_eq!(dirty.iter().collect::<Vec<_>>(), vec![4, 5]);
            }
            other => panic!("expected a Delta, got {other:?}"),
        }
    }

    #[test]
    fn a_stalled_client_is_forced_a_snapshot_after_the_timeout() {
        let (mut p, t0) = publisher(10);
        p.attach(C1, AttachMode::Active, t0);
        let mut now = settle(&mut p, C1, t0, 1);

        for seq in 2..=5 {
            p.record_damage(&dirty(10, &[0]));
            p.flush(now, Seq::new(seq));
            now += Duration::from_millis(10);
        }

        // Blocked, and the stall clock starts on the first skipped flush.
        p.record_damage(&dirty(10, &[1]));
        assert!(p.flush(now, Seq::new(6)).is_empty());
        let stalled_at = p.subscription(C1).unwrap().stalled_since().unwrap();
        assert_eq!(stalled_at, now);

        // Still inside the 3 s grace.
        now += Duration::from_millis(2_900);
        p.record_damage(&dirty(10, &[2]));
        assert!(p.flush(now, Seq::new(6)).is_empty());

        // Past it: a Snapshot is forced and the window reopens.
        now += Duration::from_millis(200);
        p.record_damage(&dirty(10, &[3]));
        let out = p.flush(now, Seq::new(6));
        assert_eq!(out[0].kind, EmissionKind::Snapshot);
        let sub = p.subscription(C1).unwrap();
        assert!(!sub.is_window_blocked(4));
        assert_eq!(sub.last_sent_seq(), Seq::new(6));
        assert!(sub.stalled_since().is_none());
    }

    #[test]
    fn a_bell_is_never_held_back_by_the_window() {
        let (mut p, t0) = publisher(10);
        p.attach(C1, AttachMode::Active, t0);
        let mut now = settle(&mut p, C1, t0, 1);
        for seq in 2..=5 {
            p.record_damage(&dirty(10, &[0]));
            p.flush(now, Seq::new(seq));
            now += Duration::from_millis(10);
        }
        p.record_bell();
        let out = p.flush(now, Seq::new(6));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, EmissionKind::BellOnly);
        assert!(out[0].bell);
    }

    #[test]
    fn the_timer_is_leading_edge_at_120_hz() {
        let (mut p, t0) = publisher(10);
        p.attach(C1, AttachMode::Active, t0);
        let now = settle(&mut p, C1, t0, 1);

        p.record_damage(&dirty(10, &[0]));
        assert!(p.should_flush(now), "20 ms after the last flush: due");
        p.flush(now, Seq::new(2));

        p.record_damage(&dirty(10, &[1]));
        assert!(!p.should_flush(now + Duration::from_millis(4)));
        assert!(p.should_flush(now + Duration::from_millis(9)));
        assert_eq!(
            p.next_flush_at(),
            Some(now + Duration::from_nanos(8_333_333))
        );

        p.flush(now + Duration::from_millis(9), Seq::new(3));
        assert_eq!(p.next_flush_at(), None, "no idle ticks");
    }

    #[test]
    fn a_resize_dirties_everything_and_rides_along() {
        let (mut p, t0) = publisher(10);
        p.attach(C1, AttachMode::Active, t0);
        let now = settle(&mut p, C1, t0, 1);

        p.record_resize(100, 40);
        let out = p.flush(now, Seq::new(2));
        match &out[0].kind {
            EmissionKind::Delta { dirty, resized, .. } => {
                assert_eq!(*resized, Some((100, 40)));
                assert_eq!(dirty.count(), 40);
            }
            other => panic!("expected a Delta, got {other:?}"),
        }
    }

    #[test]
    fn a_style_overflow_forces_snapshots_everywhere() {
        let (mut p, t0) = publisher(10);
        p.attach(C1, AttachMode::Active, t0);
        p.attach(C2, AttachMode::Passive, t0);
        let now = settle2(&mut p, t0);

        p.force_snapshot_all();
        let out = p.flush(now, Seq::new(2));
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|e| e.kind == EmissionKind::Snapshot));
    }

    fn settle2(p: &mut Publisher, now: Instant) -> Instant {
        p.flush(now, Seq::new(1));
        p.ack(C1, Seq::new(1), now);
        p.ack(C2, Seq::new(1), now);
        now + Duration::from_millis(20)
    }

    #[test]
    fn a_silent_client_is_reported_after_thirty_seconds() {
        let (mut p, t0) = publisher(10);
        p.attach(C1, AttachMode::Active, t0);
        let mut now = settle(&mut p, C1, t0, 1);

        p.record_damage(&dirty(10, &[0]));
        p.flush(now, Seq::new(2));
        assert!(p.silent_clients(now).is_empty());

        now += Duration::from_secs(29);
        assert!(p.silent_clients(now).is_empty());
        now += Duration::from_secs(2);
        assert_eq!(p.silent_clients(now), vec![C1]);

        p.ack(C1, Seq::new(2), now);
        assert!(p.silent_clients(now).is_empty());
    }

    #[test]
    fn detach_stops_everything() {
        let (mut p, t0) = publisher(10);
        p.attach(C1, AttachMode::Active, t0);
        assert_eq!(p.len(), 1);
        assert!(p.detach(C1));
        assert!(!p.detach(C1));
        assert!(p.is_empty());
        p.record_damage(&dirty(10, &[0]));
        assert!(!p.has_pending());
        assert!(p.flush(t0, Seq::new(2)).is_empty());
    }
}
