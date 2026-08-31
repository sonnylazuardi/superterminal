//! `SetViewState` — reporting scroll offset and selection to the Server
//! (grilling Q43 and Q49, data-plane msg `0x0016`).
//!
//! # Why this module has a sink instead of a socket
//!
//! Q43 routes View State over the Data Plane and Q49 froze the message, but
//! `st-client-core`'s `DataPlaneHandle` exposes no way to send it: the only
//! outbound calls are `attach`, `detach`, `send_input`, `resize`,
//! `fetch_history` and `ack`, and the socket writer behind them is private
//! (`Shared::send`). Writing to a `try_clone`d socket beside it would risk
//! interleaving a partial write into another frame and desynchronising the
//! stream, which is worse than not reporting View State at all.
//!
//! So the payload construction and the debounce policy — the parts with
//! actual logic — live here behind a [`ViewStateSink`], fully unit-tested, and
//! the transport is one `set_sink` call away. The three lines
//! `st-client-core` needs are recorded in the module docs of `lib.rs`.

use std::sync::{Mutex, OnceLock};

use st_client_core::Selection;
use st_proto::data::SetViewState;
use st_proto::{AbsLine, SurfaceId};

/// Minimum gap between two scroll-driven reports. A drag on the scrollbar
/// produces one event per pointer move; the Server only needs the last one.
pub const SCROLL_DEBOUNCE_MS: u64 = 150;

/// Somewhere for [`SetViewState`] messages to go.
///
/// `Send + Sync` because the element hands one to the process-global registry
/// and napi may read it from the JS thread.
pub trait ViewStateSink: Send + Sync {
    /// Delivers one message. Errors are logged by the caller, never fatal:
    /// View State is a convenience, not a correctness requirement.
    fn send(&self, message: SetViewState) -> Result<(), String>;
}

/// The default sink: keeps the last message per Surface so tests and
/// `get_prop("viewState")` can assert on what *would* have been sent.
#[derive(Debug, Default)]
pub struct RecordingSink {
    messages: Mutex<Vec<SetViewState>>,
}

impl RecordingSink {
    /// Every message recorded so far, oldest first.
    #[must_use]
    pub fn drain(&self) -> Vec<SetViewState> {
        std::mem::take(&mut self.messages.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The most recent message for a Surface, without consuming anything.
    #[must_use]
    pub fn last_for(&self, surface: SurfaceId) -> Option<SetViewState> {
        self.messages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .rev()
            .find(|message| message.surface == surface)
            .cloned()
    }

    /// How many messages have been recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// `true` when nothing has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ViewStateSink for RecordingSink {
    fn send(&self, message: SetViewState) -> Result<(), String> {
        let mut messages = self.messages.lock().unwrap_or_else(|e| e.into_inner());
        // Bounded: nothing drains this in production, and an unbounded Vec on
        // a scroll path is a leak.
        if messages.len() >= 64 {
            messages.remove(0);
        }
        messages.push(message);
        Ok(())
    }
}

static SINK: OnceLock<Box<dyn ViewStateSink>> = OnceLock::new();
static RECORDER: OnceLock<&'static RecordingSink> = OnceLock::new();

/// Forwards to the process-wide [`RecordingSink`], so the default sink can be
/// a `Box<dyn ViewStateSink>` without boxing a second recorder.
struct RecorderSink;

impl ViewStateSink for RecorderSink {
    fn send(&self, message: SetViewState) -> Result<(), String> {
        recorder().send(message)
    }
}

/// Installs the process-wide sink. First caller wins; returns `false` if a
/// sink was already installed.
pub fn set_sink(sink: Box<dyn ViewStateSink>) -> bool {
    SINK.set(sink).is_ok()
}

/// The installed sink, defaulting to the recorder.
#[must_use]
pub fn sink() -> &'static dyn ViewStateSink {
    SINK.get_or_init(|| Box::new(RecorderSink)).as_ref()
}

/// The default recorder, for tests and for the `viewState` read-back.
#[must_use]
pub fn recorder() -> &'static RecordingSink {
    RECORDER.get_or_init(|| Box::leak(Box::new(RecordingSink::default())))
}

/// Builds the message for a Surface's current View State.
///
/// `scroll_offset` is stored as distance from the bottom (04 §8), but the wire
/// carries the *absolute* first visible line, which is what survives a
/// reconnect after the scrollback has moved underneath it.
#[must_use]
pub fn message(
    surface: SurfaceId,
    first_visible_line: AbsLine,
    selection: Option<&Selection>,
) -> SetViewState {
    SetViewState {
        surface,
        scroll_offset: Some(first_visible_line),
        selection: selection
            .filter(|selection| !selection.is_empty())
            .map(|selection| selection.to_wire()),
    }
}

/// Decides *when* to report, so a scroll drag does not put one frame's worth
/// of messages on the socket (04 §8).
#[derive(Debug, Clone, Default)]
pub struct ViewStateDebouncer {
    last_sent_ms: Option<u64>,
    last_payload: Option<(u64, Option<st_proto::Selection>)>,
}

/// Why a report was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Mouse-up ended a selection: send now (04 §8).
    SelectionEnd,
    /// The viewport moved: coalesce.
    Scroll,
}

impl ViewStateDebouncer {
    /// `true` when the caller should send `message`. Records it as sent.
    pub fn should_send(&mut self, now_ms: u64, message: &SetViewState, trigger: Trigger) -> bool {
        let payload = (
            message.scroll_offset.map_or(u64::MAX, AbsLine::get),
            message.selection,
        );
        if self.last_payload.as_ref() == Some(&payload) {
            return false;
        }
        if trigger == Trigger::Scroll {
            if let Some(last) = self.last_sent_ms {
                if now_ms.saturating_sub(last) < SCROLL_DEBOUNCE_MS {
                    return false;
                }
            }
        }
        self.last_sent_ms = Some(now_ms);
        self.last_payload = Some(payload);
        true
    }

    /// Forgets what was sent, so the next call always reports. Used when the
    /// Surface changes or the connection was re-established.
    pub fn reset(&mut self) {
        self.last_sent_ms = None;
        self.last_payload = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use st_client_core::selection::{AbsPoint, SelectionMode};

    fn selection(line: u64, col: u16) -> Selection {
        let mut selection = Selection::new(
            AbsPoint {
                line: AbsLine::new(line),
                col,
            },
            SelectionMode::Char,
        );
        selection.extend_to(AbsPoint {
            line: AbsLine::new(line),
            col: col + 4,
        });
        selection
    }

    #[test]
    fn a_message_carries_the_absolute_first_visible_line() {
        let message = message(SurfaceId(3), AbsLine::new(1200), None);
        assert_eq!(message.surface, SurfaceId(3));
        assert_eq!(message.scroll_offset, Some(AbsLine::new(1200)));
        assert!(message.selection.is_none());
    }

    #[test]
    fn an_empty_selection_is_reported_as_no_selection() {
        let empty = Selection::new(
            AbsPoint {
                line: AbsLine::new(1),
                col: 3,
            },
            SelectionMode::Char,
        );
        assert!(empty.is_empty());
        let for_empty = message(SurfaceId(1), AbsLine::ZERO, Some(&empty));
        assert!(for_empty.selection.is_none());

        let real = selection(1, 3);
        let for_real = message(SurfaceId(1), AbsLine::ZERO, Some(&real));
        assert_eq!(for_real.selection, Some(real.to_wire()));
    }

    #[test]
    fn selection_end_is_never_debounced() {
        let mut debouncer = ViewStateDebouncer::default();
        let a = message(SurfaceId(1), AbsLine::ZERO, Some(&selection(1, 0)));
        let b = message(SurfaceId(1), AbsLine::ZERO, Some(&selection(2, 0)));
        assert!(debouncer.should_send(0, &a, Trigger::SelectionEnd));
        assert!(debouncer.should_send(1, &b, Trigger::SelectionEnd));
    }

    #[test]
    fn scrolling_is_coalesced_to_one_report_per_window() {
        let mut debouncer = ViewStateDebouncer::default();
        let first = message(SurfaceId(1), AbsLine::new(10), None);
        assert!(debouncer.should_send(1000, &first, Trigger::Scroll));
        for line in 11..30 {
            let message = message(SurfaceId(1), AbsLine::new(line), None);
            assert!(
                !debouncer.should_send(1000 + line, &message, Trigger::Scroll),
                "line {line} slipped through the debounce"
            );
        }
        let later = message(SurfaceId(1), AbsLine::new(40), None);
        assert!(debouncer.should_send(1000 + SCROLL_DEBOUNCE_MS, &later, Trigger::Scroll));
    }

    #[test]
    fn an_unchanged_payload_is_never_resent() {
        let mut debouncer = ViewStateDebouncer::default();
        let message = message(SurfaceId(1), AbsLine::new(10), None);
        assert!(debouncer.should_send(0, &message, Trigger::SelectionEnd));
        assert!(!debouncer.should_send(10_000, &message, Trigger::SelectionEnd));
        assert!(!debouncer.should_send(10_000, &message, Trigger::Scroll));
    }

    #[test]
    fn a_reset_makes_the_next_report_go_out() {
        let mut debouncer = ViewStateDebouncer::default();
        let message = message(SurfaceId(1), AbsLine::new(10), None);
        assert!(debouncer.should_send(0, &message, Trigger::Scroll));
        assert!(!debouncer.should_send(0, &message, Trigger::Scroll));
        debouncer.reset();
        assert!(debouncer.should_send(0, &message, Trigger::Scroll));
    }

    #[test]
    fn the_recording_sink_keeps_the_last_message_per_surface() {
        let sink = RecordingSink::default();
        assert!(sink.is_empty());
        sink.send(message(SurfaceId(1), AbsLine::new(1), None))
            .unwrap();
        sink.send(message(SurfaceId(2), AbsLine::new(2), None))
            .unwrap();
        sink.send(message(SurfaceId(1), AbsLine::new(3), None))
            .unwrap();
        assert_eq!(
            sink.last_for(SurfaceId(1)).unwrap().scroll_offset,
            Some(AbsLine::new(3))
        );
        assert_eq!(
            sink.last_for(SurfaceId(2)).unwrap().scroll_offset,
            Some(AbsLine::new(2))
        );
        assert_eq!(sink.drain().len(), 3);
        assert!(sink.is_empty());
    }

    #[test]
    fn the_recording_sink_is_bounded() {
        let sink = RecordingSink::default();
        for line in 0..200 {
            sink.send(message(SurfaceId(1), AbsLine::new(line), None))
                .unwrap();
        }
        assert!(sink.len() <= 64);
        assert_eq!(
            sink.last_for(SurfaceId(1)).unwrap().scroll_offset,
            Some(AbsLine::new(199))
        );
    }
}
