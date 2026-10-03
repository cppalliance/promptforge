//! Supervisor event publication and current-run cancellation.

use std::sync::{Mutex, MutexGuard, PoisonError};

use promptforge::cancel::CancelHandle;
use tokio::sync::mpsc;

use crate::transition::SupervisorEvent;

/// Capacity of the bounded operator-cancellation queue.
///
/// `OperatorCancellation` is the one loss-tolerant supervisor event: a
/// full queue already holds a pending cancellation that retires the
/// current run, so dropping a concurrent duplicate preserves semantics.
/// Producers are operator gestures, so one pending cancellation covers the
/// entire in-flight set with headroom.
pub const CANCELLATION_CAPACITY: usize = 1;

/// Synchronous producers for one supervisor's typed event stream.
#[derive(Debug)]
pub struct RunLifecycle {
    /// The current run's cancellation handle.
    cancel: Mutex<CancelHandle>,
    events: mpsc::UnboundedSender<SupervisorEvent>,
    cancellations: mpsc::Sender<SupervisorEvent>,
}

impl RunLifecycle {
    /// Creates the lifecycle over the supervisor's event senders: the
    /// unbounded queue holds close, the loss-intolerant event, and the
    /// bounded queue holds operator cancellations.
    #[must_use]
    pub fn new(
        events: mpsc::UnboundedSender<SupervisorEvent>,
        cancellations: mpsc::Sender<SupervisorEvent>,
    ) -> Self {
        Self {
            cancel: Mutex::new(CancelHandle::new()),
            events,
            cancellations,
        }
    }

    /// Locks the current cancellation handle, recovering from a panicking
    /// peer.
    fn lock(&self) -> MutexGuard<'_, CancelHandle> {
        self.cancel.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Arms a fresh cancellation handle for the next run and returns it:
    /// the handle is the only way the armed run observes a later cancel.
    /// It is the Engine's own flag, so the run's context polls it and the
    /// effect loop awaits it with no bridge between.
    #[must_use]
    pub fn arm(&self) -> CancelHandle {
        let fresh = CancelHandle::new();
        *self.lock() = fresh.clone();
        fresh
    }

    /// Publishes an operator cancellation for reducer ownership.
    ///
    /// Loss-tolerant by design: when the bounded queue is full it already
    /// holds a pending cancellation that retires the current run, so a
    /// concurrent duplicate is dropped rather than queued.
    pub fn operator_cancel(&self) {
        let _ = self
            .cancellations
            .try_send(SupervisorEvent::OperatorCancellation);
    }

    /// Cancels the reducer-owned current run.
    pub fn cancel_current(&self) {
        self.lock().cancel();
    }

    /// Publishes session close for reducer ownership.
    pub fn close(&self) {
        self.send(SupervisorEvent::Close);
    }

    /// Sends one loss-intolerant event; a gone receiver means supervision
    /// already ended. Close is sent on the unbounded queue because its
    /// loss would leave a closed session supervised, and the supervisor
    /// stops reading once it has handled the first one.
    fn send(&self, event: SupervisorEvent) {
        let _ = self.events.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lifecycle() -> (
        RunLifecycle,
        mpsc::UnboundedReceiver<SupervisorEvent>,
        mpsc::Receiver<SupervisorEvent>,
    ) {
        let (events, guaranteed) = mpsc::unbounded_channel();
        let (cancellations, bounded) = mpsc::channel(CANCELLATION_CAPACITY);
        (
            RunLifecycle::new(events, cancellations),
            guaranteed,
            bounded,
        )
    }

    #[test]
    fn operator_cancellations_never_grow_the_bounded_queue_past_capacity() {
        let (lifecycle, _guaranteed, mut bounded) = lifecycle();

        for _ in 0..8 {
            lifecycle.operator_cancel();
        }

        let mut received = 0;
        while bounded.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(
            received, CANCELLATION_CAPACITY,
            "a full cancellation queue drops redundant duplicates instead of growing"
        );
    }

    #[test]
    fn guaranteed_events_flow_past_a_full_cancellation_queue() {
        let (lifecycle, mut guaranteed, _bounded) = lifecycle();
        for _ in 0..4 {
            lifecycle.operator_cancel();
        }

        lifecycle.close();

        assert_eq!(
            guaranteed.try_recv().expect("close is delivered"),
            SupervisorEvent::Close,
            "loss-intolerant events keep guaranteed delivery when cancellations overflow"
        );
    }
}
