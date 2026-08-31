//! Bounded tracking and cancellation for inference requests during switches.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

/// Time allowed after cancellation for request futures to drop their guards.
const CANCELLATION_GRACE: Duration = Duration::from_secs(1);

/// Inference requests registered while holding the profile-switch lock.
#[derive(Debug, Default)]
pub(crate) struct InFlight {
    next_id: AtomicU64,
    requests: Mutex<HashMap<u64, Arc<RequestCancellation>>>,
    changed: Notify,
}

impl InFlight {
    /// Registers one request until the returned guard drops.
    pub(crate) fn register(self: &Arc<Self>) -> InFlightGuard {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let cancellation = Arc::new(RequestCancellation::default());
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, Arc::clone(&cancellation));
        InFlightGuard {
            id,
            tracker: Arc::clone(self),
            cancellation,
        }
    }

    /// Waits at most `timeout` for all registered requests to finish.
    ///
    /// Returns `true` when every request completed and `false` on timeout.
    pub(crate) async fn drain(&self, timeout: Duration) -> bool {
        tokio::time::timeout(timeout, self.wait_empty())
            .await
            .is_ok()
    }

    /// Drains naturally until `timeout`, then cancels every straggler and
    /// gives request futures a short bounded window to release their guards.
    ///
    /// Returns `true` when all guards dropped before either deadline.
    pub(crate) async fn drain_or_cancel(&self, timeout: Duration) -> bool {
        if self.drain(timeout).await {
            return true;
        }
        self.cancel_all();
        self.drain(CANCELLATION_GRACE).await
    }

    /// Signals every request still registered after the drain deadline.
    pub(crate) fn cancel_all(&self) {
        let requests: Vec<Arc<RequestCancellation>> = self
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for request in requests {
            request.cancel();
        }
    }

    async fn wait_empty(&self) {
        loop {
            let changed = self.changed.notified();
            if self
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
            {
                return;
            }
            changed.await;
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

/// Lifetime and cancellation handle for one tracked inference request.
#[derive(Debug)]
pub(crate) struct InFlightGuard {
    id: u64,
    tracker: Arc<InFlight>,
    cancellation: Arc<RequestCancellation>,
}

impl InFlightGuard {
    /// Resolves when a profile switch cancels this request.
    pub(crate) async fn cancelled(&self) {
        self.cancellation.cancelled().await;
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.tracker
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
        self.tracker.changed.notify_waiters();
    }
}

#[derive(Debug, Default)]
struct RequestCancellation {
    cancelled: AtomicBool,
    notify: Notify,
}

impl RequestCancellation {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn cancelled(&self) {
        let notified = self.notify.notified();
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        notified.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn bounded_drain_cancels_a_straggler() {
        let tracker = Arc::new(InFlight::default());
        let request = tracker.register();
        let started = tokio::time::Instant::now();
        let cancelled = tokio::spawn(async move {
            request.cancelled().await;
            drop(request);
        });

        assert!(tracker.drain_or_cancel(Duration::from_secs(30)).await);
        cancelled.await.expect("request task joins");
        assert_eq!(started.elapsed(), Duration::from_secs(30));
        assert_eq!(tracker.len(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_grace_keeps_a_stubborn_guard_bounded() {
        let tracker = Arc::new(InFlight::default());
        let request = tracker.register();
        let started = tokio::time::Instant::now();

        assert!(!tracker.drain_or_cancel(Duration::from_secs(30)).await);
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(30) + CANCELLATION_GRACE
        );
        assert_eq!(tracker.len(), 1);
        drop(request);
    }

    #[tokio::test]
    async fn completed_requests_leave_the_drain_immediately() {
        let tracker = Arc::new(InFlight::default());
        drop(tracker.register());

        assert!(tracker.drain(Duration::from_secs(30)).await);
    }
}
