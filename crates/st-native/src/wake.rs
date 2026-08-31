//! Waking GPUI from the Data Plane thread (04 §5, HANDOVER V4).
//!
//! `AsyncApp` holds a `Weak<AppCell>` and is therefore `!Send`, so the
//! "capture an `AsyncApp` and call `cx.notify()`" plan from 04 §5 cannot be
//! used verbatim. What *is* portable is the trick gpuix itself uses to reach
//! the GPUI thread from Bun's thread (`renderer.rs::run_ui_commands`): a
//! `futures::channel::mpsc` sender, which is `Send + Sync`, drained by a task
//! spawned on GPUI's foreground executor. Delivering into the channel wakes
//! that task, the task pings the platform run loop, and `cx.notify()` happens
//! on the right thread on both macOS (`CFRunLoop`) and Linux (`calloop`).
//!
//! Coalescing (grilling Q27): the sender is guarded by an `AtomicBool`, so N
//! Deltas landing between two frames cost one `notify`. The flag is cleared by
//! the drain task *before* it notifies, so a Delta that arrives while the
//! frame is being built schedules the next one instead of being swallowed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use st_client_core::WakeFn;

/// The GPUI-thread end of the wake channel.
pub type WakeReceiver = UnboundedReceiver<()>;

/// The Data-Plane-thread end, kept so the element can wake itself (after a
/// local scroll, say) through exactly the same path.
#[derive(Clone, Debug)]
pub struct Waker {
    tx: UnboundedSender<()>,
    pending: Arc<AtomicBool>,
}

impl Waker {
    /// Requests a repaint. Cheap and idempotent between frames.
    pub fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            // A closed channel means the window is gone; the next render will
            // rebuild the pair, and dropping the wake is the correct answer.
            let _ = self.tx.unbounded_send(());
        }
    }

    /// Clears the coalescing flag. The drain task calls this immediately
    /// before notifying, never after.
    pub fn armed(&self) {
        self.pending.store(false, Ordering::Release);
    }

    /// `true` when a repaint has been requested and not yet served.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }

    /// The boxed callback `st_client_core::dataplane` wants.
    #[must_use]
    pub fn as_wake_fn(&self) -> WakeFn {
        let waker = self.clone();
        Box::new(move || waker.wake())
    }
}

/// A fresh wake channel.
#[must_use]
pub fn channel() -> (Waker, WakeReceiver) {
    let (tx, rx) = unbounded();
    (
        Waker {
            tx,
            pending: Arc::new(AtomicBool::new(false)),
        },
        rx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn drain(rx: &mut WakeReceiver) -> usize {
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        count
    }

    #[test]
    fn many_wakes_between_frames_cost_one_notify() {
        let (waker, mut rx) = channel();
        for _ in 0..100 {
            waker.wake();
        }
        assert_eq!(drain(&mut rx), 1);
        assert!(waker.is_pending());
    }

    #[test]
    fn the_next_frame_wakes_again_once_the_flag_is_cleared() {
        let (waker, mut rx) = channel();
        waker.wake();
        assert_eq!(drain(&mut rx), 1);
        waker.armed();
        assert!(!waker.is_pending());
        waker.wake();
        assert_eq!(drain(&mut rx), 1);
    }

    #[test]
    fn a_wake_from_another_thread_arrives() {
        let (waker, mut rx) = channel();
        let wake_fn = waker.as_wake_fn();
        std::thread::spawn(wake_fn).join().unwrap();
        assert_eq!(drain(&mut rx), 1);
    }

    #[test]
    fn a_dropped_receiver_does_not_panic_the_data_plane_thread() {
        let (waker, rx) = channel();
        drop(rx);
        waker.wake();
        waker.armed();
        waker.wake();
    }

    #[test]
    fn the_wake_fn_is_send_and_sync_as_the_data_plane_requires() {
        fn assert_wake_fn(_: &WakeFn) {}
        let (waker, _rx) = channel();
        let wake_fn = waker.as_wake_fn();
        assert_wake_fn(&wake_fn);
    }
}
