//! Supervisor event publication and current-run cancellation.

use std::sync::{Mutex, MutexGuard, PoisonError};

use promptforge_core_support::cancel::CancelHandle;
use tokio::sync::mpsc;

use super::supervisor::transition::{RunId, SupervisorEvent};

/// Capacity of the bounded operator-cancellation queue.
///
/// `OperatorCancellation` is the one loss-tolerant supervisor event: no
/// reducer wait is ever conditioned on it, and a full queue already holds
/// a pending cancellation that retires the current run, so dropping a
/// concurrent duplicate preserves semantics. Producers are operator
/// gestures, so one pending cancellation covers the entire in-flight set
/// with headroom.
pub(super) const CANCELLATION_CAPACITY: usize = 1;

/// Synchronous producers for one supervisor's typed event stream.
pub(super) struct RunLifecycle {
    state: Mutex<RunState>,
    events: mpsc::UnboundedSender<SupervisorEvent>,
    cancellations: mpsc::Sender<SupervisorEvent>,
}

/// The current run identity and cancellation handle.
struct RunState {
    cancel: CancelHandle,
    run: Option<RunId>,
}

impl RunLifecycle {
    /// Creates the lifecycle over the supervisor's event senders: the
    /// unbounded queue carries the loss-intolerant events the reducer
    /// waits on, the bounded queue carries operator cancellations.
    pub(super) fn new(
        events: mpsc::UnboundedSender<SupervisorEvent>,
        cancellations: mpsc::Sender<SupervisorEvent>,
    ) -> Self {
        Self {
            state: Mutex::new(RunState {
                cancel: CancelHandle::new(),
                run: None,
            }),
            events,
            cancellations,
        }
    }

    /// Locks lifecycle state, recovering from a panicking peer.
    fn lock(&self) -> MutexGuard<'_, RunState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Arms the cancellation handle for `run`.
    pub(super) fn arm(&self, run: RunId) -> CancelHandle {
        let fresh = CancelHandle::new();
        let mut state = self.lock();
        state.cancel = fresh.clone();
        state.run = Some(run);
        fresh
    }

    /// Publishes an operator cancellation for reducer ownership.
    ///
    /// Loss-tolerant by design: when the bounded queue is full it already
    /// holds a pending cancellation that retires the current run, so a
    /// concurrent duplicate is dropped rather than queued.
    pub(super) fn operator_cancel(&self) {
        let _ = self
            .cancellations
            .try_send(SupervisorEvent::OperatorCancellation);
    }

    /// Publishes that input resumed the currently armed run.
    pub(super) fn accept_input(&self) -> Option<RunId> {
        let run = self.lock().run?;
        self.send(SupervisorEvent::AcceptedInput(run));
        Some(run)
    }

    /// Publishes a durable terminal event for the currently armed run.
    pub(super) fn settle_current_turn(&self) {
        if let Some(run) = self.lock().run {
            self.settle_turn(run);
        }
    }

    /// Publishes a terminal event scoped to `run`.
    pub(super) fn settle_turn(&self, run: RunId) {
        self.send(SupervisorEvent::TerminalSettlement(run));
    }

    /// Cancels the reducer-owned current run.
    pub(super) fn cancel_current(&self) {
        self.lock().cancel.cancel();
    }

    /// Clears `run` after its future completes or is dropped.
    pub(super) fn finish(&self, run: RunId) {
        let mut state = self.lock();
        if state.run == Some(run) {
            state.run = None;
        }
    }

    /// Publishes session close for reducer ownership.
    pub(super) fn close(&self) {
        self.send(SupervisorEvent::Close);
    }

    /// Sends one loss-intolerant event; a gone receiver means supervision
    /// already ended. These events ride the unbounded queue because the
    /// reducer awaits settlements and close, so their loss could hang a
    /// state transition, and their volume is bounded by armed runs and
    /// durable turns rather than by caller repetition.
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
