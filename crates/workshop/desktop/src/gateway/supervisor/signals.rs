//! Stop and completion signals for bounded supervisor shutdown.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Instant;

/// A stop request that never waits for in-progress supervisor work.
#[derive(Clone, Debug, Default)]
pub(super) struct StopSignal {
    state: Arc<StopState>,
}

#[derive(Debug, Default)]
struct StopState {
    requested: AtomicBool,
    waiter: Mutex<()>,
    wake: Condvar,
}

impl StopSignal {
    pub(super) fn signal(&self) {
        let waiter = self
            .state
            .waiter
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.state.requested.store(true, Ordering::SeqCst);
        self.state.wake.notify_all();
        drop(waiter);
    }

    pub(super) fn wait(&self) {
        if self.state.requested.load(Ordering::SeqCst) {
            return;
        }
        let waiter = self
            .state
            .waiter
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        drop(
            self.state
                .wake
                .wait_while(waiter, |()| !self.state.requested.load(Ordering::SeqCst))
                .unwrap_or_else(PoisonError::into_inner),
        );
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Completion {
    state: Arc<CompletionState>,
}

#[derive(Debug, Default)]
struct CompletionState {
    finished: Mutex<bool>,
    wake: Condvar,
}

impl Completion {
    pub(super) fn guard(&self) -> CompletionGuard {
        CompletionGuard(self.clone())
    }

    pub(super) fn wait_until(&self, deadline: Instant) -> bool {
        let mut finished = self
            .state
            .finished
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            if *finished {
                return true;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            if remaining.is_zero() {
                return false;
            }
            let (next, timeout) = self
                .state
                .wake
                .wait_timeout(finished, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            finished = next;
            if timeout.timed_out() && !*finished {
                return false;
            }
        }
    }

    #[cfg(test)]
    pub(super) fn wake(&self) {
        self.state.wake.notify_all();
    }
}

pub(super) struct CompletionGuard(Completion);

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        *self
            .0
            .state
            .finished
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = true;
        self.0.state.wake.notify_all();
    }
}
