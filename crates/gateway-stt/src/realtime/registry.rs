use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

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
    retiring: Vec<RetiringSession>,
}

impl RegistryState {
    fn reap_retired(&mut self) {
        let before = self.retiring.len();
        self.retiring.retain_mut(|session| !session.joined());
        self.active = self
            .active
            .saturating_sub(before.saturating_sub(self.retiring.len()));
    }
}

impl fmt::Debug for RegistryState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegistryState")
            .field("active", &self.active)
            .field("retiring", &self.retiring.len())
            .finish()
    }
}

trait RetiredTask: Send {
    fn poll_join(&mut self) -> Poll<()>;
}

impl<T> RetiredTask for JoinHandle<T>
where
    T: Send + 'static,
{
    fn poll_join(&mut self) -> Poll<()> {
        let waker = futures_util::task::noop_waker_ref();
        let mut context = Context::from_waker(waker);
        Pin::new(self).poll(&mut context).map(|_result| ())
    }
}

struct RetiringSession {
    tasks: Vec<Box<dyn RetiredTask>>,
}

impl RetiringSession {
    fn joined(&mut self) -> bool {
        let mut index = 0;
        while index < self.tasks.len() {
            if self.tasks[index].poll_join().is_ready() {
                drop(self.tasks.remove(index));
            } else {
                index += 1;
            }
        }
        self.tasks.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SessionRegistry {
    state: Arc<Mutex<RegistryState>>,
}

impl SessionRegistry {
    pub(crate) fn register(&self) -> Result<SessionRegistration, RegisterError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.reap_retired();
        if state.active == MAX_ACTIVE_REALTIME_SESSIONS {
            return Err(RegisterError::AtCapacity);
        }
        state.active += 1;
        Ok(SessionRegistration {
            state: Some(Arc::clone(&self.state)),
        })
    }

    pub(crate) fn active(&self) -> usize {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.reap_retired();
        state.active
    }
}

#[derive(Debug)]
pub(crate) struct SessionRegistration {
    state: Option<Arc<Mutex<RegistryState>>>,
}

impl SessionRegistration {
    pub(crate) fn retire<T>(&mut self, mut tasks: Vec<JoinHandle<T>>)
    where
        T: Send + 'static,
    {
        for task in &tasks {
            task.abort();
        }
        if tasks.is_empty() {
            return;
        }
        let Some(state) = self.state.take() else {
            return;
        };
        state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retiring
            .push(RetiringSession {
                tasks: tasks
                    .drain(..)
                    .map(|task| Box::new(task) as Box<dyn RetiredTask>)
                    .collect(),
            });
        // The registry keeps this admission occupied until reap_retired
        // polls every canceled task join to completion.
        drop(state);
    }
}

impl Drop for SessionRegistration {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
        debug_assert!(state.active > 0);
        state.active = state.active.saturating_sub(1);
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
