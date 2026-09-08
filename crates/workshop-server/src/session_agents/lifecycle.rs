//! Supervisor event publication and current-run cancellation.

use std::sync::{Mutex, MutexGuard, PoisonError};

use promptforge_core_support::cancel::CancelHandle;
use tokio::sync::mpsc;

use super::supervisor::transition::{RunId, SupervisorEvent};

/// Synchronous producers for one supervisor's typed event stream.
pub(super) struct RunLifecycle {
    state: Mutex<RunState>,
    events: mpsc::UnboundedSender<SupervisorEvent>,
}

/// The current run identity and cancellation handle.
struct RunState {
    cancel: CancelHandle,
    run: Option<RunId>,
}

impl RunLifecycle {
    /// Creates the lifecycle over the supervisor's event sender.
    pub(super) fn new(events: mpsc::UnboundedSender<SupervisorEvent>) -> Self {
        Self {
            state: Mutex::new(RunState {
                cancel: CancelHandle::new(),
                run: None,
            }),
            events,
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
    pub(super) fn operator_cancel(&self) {
        self.send(SupervisorEvent::OperatorCancellation);
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

    /// Sends one event; a gone receiver means supervision already ended.
    fn send(&self, event: SupervisorEvent) {
        let _ = self.events.send(event);
    }
}
