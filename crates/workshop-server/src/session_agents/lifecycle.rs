//! Cancellation provenance and accepted-turn settlement.

use std::sync::{Mutex, MutexGuard, PoisonError};

use promptforge_core_support::cancel::CancelHandle;
use tokio::sync::Notify;

/// Why the current run's cancellation handle fired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CancelOrigin {
    /// The operator explicitly cancelled the current turn.
    Operator,
    /// The supervisor retired an idle run for a new catalog generation.
    Catalog,
}

/// State shared by input acceptance, the supervisor, and terminal events.
pub(super) struct RunLifecycle {
    state: Mutex<RunState>,
    settled: Notify,
}

/// The current run's cancellation and accepted-turn state.
pub(super) struct RunState {
    cancel: CancelHandle,
    origin: Option<CancelOrigin>,
    pub(super) accepted_turn: bool,
}

impl RunLifecycle {
    /// Creates the lifecycle before the first run is armed.
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(RunState {
                cancel: CancelHandle::new(),
                origin: None,
                accepted_turn: false,
            }),
            settled: Notify::new(),
        }
    }

    /// Locks lifecycle state, recovering from a panicking peer.
    pub(super) fn lock(&self) -> MutexGuard<'_, RunState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Arms a fresh run and clears the prior stop provenance.
    pub(super) fn arm(&self) -> CancelHandle {
        let fresh = CancelHandle::new();
        let mut state = self.lock();
        state.cancel = fresh.clone();
        state.origin = None;
        state.accepted_turn = false;
        fresh
    }

    /// Cancels immediately for an operator request.
    pub(super) fn cancel(&self, origin: CancelOrigin) {
        let mut state = self.lock();
        state.origin = Some(origin);
        state.accepted_turn = false;
        state.cancel.cancel();
        self.settled.notify_waiters();
    }

    /// Cancels for catalog replacement only when no accepted turn is active.
    pub(super) fn cancel_for_catalog(&self) -> bool {
        let mut state = self.lock();
        if state.accepted_turn {
            return false;
        }
        state.origin = Some(CancelOrigin::Catalog);
        state.cancel.cancel();
        true
    }

    /// Records that the accepted turn reached a durable terminal event.
    pub(super) fn settle_turn(&self) {
        let mut state = self.lock();
        if state.accepted_turn {
            state.accepted_turn = false;
            self.settled.notify_waiters();
        }
    }

    /// Waits cancellation-safely until no accepted turn remains.
    pub(super) async fn wait_until_settled(&self) {
        loop {
            let notified = self.settled.notified();
            if !self.lock().accepted_turn {
                return;
            }
            notified.await;
        }
    }

    /// Returns the current run's cancellation provenance.
    pub(super) fn origin(&self) -> Option<CancelOrigin> {
        self.lock().origin
    }
}
