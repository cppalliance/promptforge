#[cfg(feature = "test-fixtures")]
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::Notify;
use tokio::task::JoinHandle;

pub(crate) const MAX_ACTIVE_REALTIME_SESSIONS: usize = 8;

#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub(crate) enum RegisterError {
    #[error("the realtime transcription session limit is reached")]
    AtCapacity,
}

#[derive(Default)]
struct RegistryState {
    active: usize,
    retired_task_failures: usize,
}

#[derive(Debug, Default)]
struct CleanupSignal {
    generation: AtomicUsize,
    notified: Notify,
}

impl CleanupSignal {
    fn emit(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.notified.notify_waiters();
    }

    #[cfg(feature = "test-fixtures")]
    fn event_count(&self) -> usize {
        self.generation.load(Ordering::Acquire)
    }

    #[cfg(feature = "test-fixtures")]
    fn notified(&self) -> impl Future<Output = ()> + '_ {
        let observed = self.generation.load(Ordering::Acquire);
        async move {
            loop {
                let notified = self.notified.notified();
                if self.generation.load(Ordering::Acquire) != observed {
                    return;
                }
                notified.await;
            }
        }
    }
}

#[derive(Debug, Default)]
struct RegistryShared {
    state: Mutex<RegistryState>,
    cleanup: CleanupSignal,
}

impl RegistryShared {
    fn release_admission(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        debug_assert!(state.active > 0);
        state.active = state.active.saturating_sub(1);
    }

    fn record_retired_task_failures(&self, failures: usize) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.retired_task_failures = state.retired_task_failures.saturating_add(failures);
    }

    fn retire<T, U>(
        self: Arc<Self>,
        interim_tasks: Vec<JoinHandle<T>>,
        finalization_tasks: Vec<JoinHandle<U>>,
    ) where
        T: Send + 'static,
        U: Send + 'static,
    {
        tokio::spawn(async move {
            let failures = join_retired_tasks(interim_tasks).await
                + join_retired_tasks(finalization_tasks).await;
            if failures != 0 {
                self.record_retired_task_failures(failures);
            }
            self.release_admission();
            self.cleanup.emit();
        });
    }
}

async fn join_retired_tasks<T>(tasks: Vec<JoinHandle<T>>) -> usize {
    let mut failures = 0usize;
    for task in tasks {
        if let Err(error) = task.await
            && !error.is_cancelled()
        {
            // Retirement aborts intentionally produce cancellation; only a panic violates cleanup.
            failures = failures.saturating_add(1);
        }
    }
    failures
}

impl std::fmt::Debug for RegistryState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegistryState")
            .field("active", &self.active)
            .field("retired_task_failures", &self.retired_task_failures)
            .finish()
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SessionRegistry {
    shared: Arc<RegistryShared>,
}

impl SessionRegistry {
    pub(crate) fn register(&self) -> Result<SessionRegistration, RegisterError> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.active == MAX_ACTIVE_REALTIME_SESSIONS {
            return Err(RegisterError::AtCapacity);
        }
        state.active += 1;
        Ok(SessionRegistration {
            shared: Some(Arc::clone(&self.shared)),
        })
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn active(&self) -> usize {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn cleanup_event_count(&self) -> usize {
        self.shared.cleanup.event_count()
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn retired_task_failures(&self) -> usize {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retired_task_failures
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn cleanup_notified(&self) -> impl Future<Output = ()> + '_ {
        self.shared.cleanup.notified()
    }
}

#[derive(Debug)]
pub(crate) struct SessionRegistration {
    shared: Option<Arc<RegistryShared>>,
}

impl SessionRegistration {
    pub(crate) fn retire<T, U>(
        &mut self,
        interim_tasks: Vec<JoinHandle<T>>,
        finalization_tasks: Vec<JoinHandle<U>>,
    ) where
        T: Send + 'static,
        U: Send + 'static,
    {
        for task in &interim_tasks {
            task.abort();
        }
        for task in &finalization_tasks {
            task.abort();
        }
        if interim_tasks.is_empty() && finalization_tasks.is_empty() {
            return;
        }
        let Some(shared) = self.shared.take() else {
            return;
        };
        shared.retire(interim_tasks, finalization_tasks);
    }
}

impl Drop for SessionRegistration {
    fn drop(&mut self) {
        let Some(shared) = self.shared.take() else {
            return;
        };
        shared.release_admission();
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_ACTIVE_REALTIME_SESSIONS, RegisterError, SessionRegistry};

    #[test]
    fn miri_registry_accepts_exact_capacity_and_rejects_capacity_plus_one() {
        assert_eq!(MAX_ACTIVE_REALTIME_SESSIONS, 8);
        let registry = SessionRegistry::default();
        let registrations = (0..MAX_ACTIVE_REALTIME_SESSIONS)
            .map(|_| registry.register().expect("capacity is admitted"))
            .collect::<Vec<_>>();

        assert_eq!(registry.active(), MAX_ACTIVE_REALTIME_SESSIONS);
        assert!(matches!(
            registry.register(),
            Err(RegisterError::AtCapacity)
        ));
        drop(registrations);
        assert_eq!(registry.active(), 0);
    }

    #[test]
    fn dropped_registration_immediately_reopens_admission() {
        let registry = SessionRegistry::default();
        let mut registrations = (0..MAX_ACTIVE_REALTIME_SESSIONS)
            .map(|_| registry.register().expect("capacity is admitted"))
            .collect::<Vec<_>>();
        drop(registrations.pop());

        let replacement = registry.register().expect("released slot is reused");
        assert_eq!(registry.active(), MAX_ACTIVE_REALTIME_SESSIONS);
        drop(replacement);
    }
}
