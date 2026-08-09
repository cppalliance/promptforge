//! Cooperative cancellation for long-running execute paths.
//!
//! Fanout runs inside [`tokio::task::block_in_place`], so dropping the outer
//! `select!` on Ctrl-C does not stop in-flight arms. Hosts install a
//! [`CancelHandle`] with [`scope`] and call [`CancelHandle::cancel`] from a
//! Ctrl-C task; fanout and model turns poll [`wait_cancelled`].

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

tokio::task_local! {
    static CURRENT: CancelHandle;
}

/// A cloneable flag that wakes waiters when cancelled.
#[derive(Clone, Debug, Default)]
pub struct CancelHandle {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancelHandle {
    /// Creates a handle that is not yet cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks this handle cancelled and wakes every waiter.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Returns whether [`Self::cancel`] has been called.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Completes when this handle is cancelled.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        loop {
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
            if self.is_cancelled() {
                return;
            }
        }
    }
}

/// Runs `fut` with `cancel` installed for [`wait_cancelled`] on this task.
pub async fn scope<F, T>(cancel: CancelHandle, fut: F) -> T
where
    F: Future<Output = T>,
{
    CURRENT.scope(cancel, fut).await
}

/// Completes when the task-local [`CancelHandle`] is cancelled.
///
/// When no handle is installed, the future never completes (hosts that do not
/// wire Ctrl-C keep prior behavior).
pub async fn wait_cancelled() {
    match CURRENT.try_with(Clone::clone) {
        Ok(handle) => handle.cancelled().await,
        Err(_) => std::future::pending::<()>().await,
    }
}

/// Returns whether the task-local handle is cancelled (false if none installed).
#[must_use]
pub fn is_cancelled() -> bool {
    CURRENT
        .try_with(CancelHandle::is_cancelled)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cancel_wakes_waiter() {
        let handle = CancelHandle::new();
        let waiter = handle.clone();
        let join = tokio::spawn(async move {
            waiter.cancelled().await;
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!handle.is_cancelled());
        handle.cancel();
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("waiter must finish after cancel")
            .expect("join ok");
        assert!(handle.is_cancelled());
    }

    #[tokio::test]
    async fn scope_exposes_handle_to_wait_cancelled() {
        let handle = CancelHandle::new();
        let cancel = handle.clone();
        let done = tokio::spawn(async move {
            scope(handle, async {
                wait_cancelled().await;
            })
            .await;
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), done)
            .await
            .expect("scoped wait must finish")
            .expect("join ok");
    }
}
