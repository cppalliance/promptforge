//! Cooperative cancellation for synchronous sidecar work.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

/// A clonable cancellation signal for finite sidecar operations.
///
/// Cancellation wakes retry waits immediately. [`run_if_active`](Self::run_if_active)
/// also provides a linearization point for effects such as process launch and
/// authoritative publication: cancellation first wakes a cancellable effect
/// already in progress, then waits for it, and no effect starts after
/// cancellation returns.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    state: Arc<CancellationState>,
}

#[derive(Debug, Default)]
struct CancellationState {
    cancelled: AtomicBool,
    effect: Mutex<()>,
    waiter: Mutex<()>,
    wake: Condvar,
}

impl CancellationToken {
    /// Creates an active cancellation token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels the token and wakes every retry wait.
    pub fn cancel(&self) {
        let waiter = self
            .state
            .waiter
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.state.cancelled.store(true, Ordering::SeqCst);
        self.state.wake.notify_all();
        drop(waiter);

        // Wait for an operation that already crossed the effect gate. The
        // cancellation flag and wake happen first, so a cancellable operation
        // inside the gate can finish instead of deadlocking with `cancel`.
        drop(
            self.state
                .effect
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }

    /// Waits for cancellation or until `timeout` elapses.
    ///
    /// Returns `true` when the token is cancelled.
    #[must_use]
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        if self.is_cancelled() {
            return true;
        }
        let waiter = self
            .state
            .waiter
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.is_cancelled() {
            return true;
        }
        let (_waiter, _) = self
            .state
            .wake
            .wait_timeout_while(waiter, timeout, |()| !self.is_cancelled())
            .unwrap_or_else(PoisonError::into_inner);
        self.is_cancelled()
    }

    /// Runs `operation` only while the token remains active.
    ///
    /// Cancellation and effect admission are mutually exclusive. Cancellation
    /// is signalled before waiting for an admitted operation, so that operation
    /// may call [`wait_timeout`](Self::wait_timeout) and stop within its bound.
    /// The gate is not reentrant: `operation` must not call `cancel` or
    /// `run_if_active` on this token.
    pub fn run_if_active<T>(&self, operation: impl FnOnce() -> T) -> Option<T> {
        let effect = self
            .state
            .effect
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.is_cancelled() {
            return None;
        }
        let result = operation();
        drop(effect);
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Instant;

    #[test]
    fn cancellation_wakes_an_operation_inside_the_effect_gate() {
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let effects = Arc::new(AtomicUsize::new(0));
        let worker_effects = Arc::clone(&effects);
        let (entered, blocked) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            worker_cancellation.run_if_active(|| {
                entered.send(()).expect("announce the gated operation");
                if !worker_cancellation.wait_timeout(Duration::from_secs(1)) {
                    worker_effects.fetch_add(1, Ordering::SeqCst);
                }
            });
        });
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("the operation enters the effect gate");

        let started = Instant::now();
        cancellation.cancel();
        worker.join().expect("the cancelled operation joins");

        assert!(
            started.elapsed() < Duration::from_millis(250),
            "cancellation wakes an operation already inside the effect gate"
        );
        assert_eq!(
            effects.load(Ordering::SeqCst),
            0,
            "the cancelled operation performs no later effect"
        );
    }
}
