//! Serialized generation replacement and explicit work ownership.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Instant;

use tokio::sync::Notify;

#[derive(Debug, Default)]
struct CoordinatorState {
    active: Option<Arc<PermitIdentity>>,
    valid: bool,
    shutting_down: bool,
}

#[derive(Debug)]
struct PermitIdentity;

/// One service-wide replacement lane.
#[derive(Debug, Default)]
pub(crate) struct ReplacementCoordinator {
    state: Mutex<CoordinatorState>,
    changed: Condvar,
}

impl ReplacementCoordinator {
    pub(crate) fn acquire(self: &Arc<Self>) -> ReplacementPermit {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.active.is_some() || state.shutting_down {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        let identity = Arc::new(PermitIdentity);
        state.active = Some(Arc::clone(&identity));
        state.valid = true;
        ReplacementPermit {
            coordinator: Arc::clone(self),
            identity: Some(identity),
        }
    }

    pub(crate) fn begin_shutdown(self: &Arc<Self>) -> ShutdownPermit {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.shutting_down {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.shutting_down = true;
        state.valid = false;
        ShutdownPermit {
            coordinator: Arc::clone(self),
        }
    }
}

/// Exclusive ownership of one staged replacement transaction.
pub(crate) struct ReplacementPermit {
    coordinator: Arc<ReplacementCoordinator>,
    identity: Option<Arc<PermitIdentity>>,
}

impl fmt::Debug for ReplacementPermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplacementPermit")
            .field("owned", &self.identity.is_some())
            .finish_non_exhaustive()
    }
}

impl ReplacementPermit {
    pub(crate) fn with_current<T>(&self, operation: impl FnOnce() -> T) -> Option<T> {
        let identity = self.identity.as_ref()?;
        let state = self
            .coordinator
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let current = state
            .active
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, identity))
            && state.valid
            && !state.shutting_down;
        current.then(operation)
    }

    pub(crate) fn is_current(&self) -> bool {
        self.with_current(|| ()).is_some()
    }
}

impl Drop for ReplacementPermit {
    fn drop(&mut self) {
        let Some(identity) = self.identity.take() else {
            return;
        };
        let mut state = self
            .coordinator
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state
            .active
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, &identity))
        {
            state.active = None;
            state.valid = false;
        }
        drop(state);
        self.coordinator.changed.notify_all();
    }
}

pub(crate) struct ShutdownPermit {
    coordinator: Arc<ReplacementCoordinator>,
}

impl Drop for ShutdownPermit {
    fn drop(&mut self) {
        let mut state = self
            .coordinator
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.shutting_down = false;
        drop(state);
        self.coordinator.changed.notify_all();
    }
}

#[derive(Debug)]
struct EpochState {
    id: u64,
    cancelled: AtomicBool,
    changed: Notify,
}

/// One replaceable cancellation epoch shared by admitted session work.
#[derive(Clone, Debug)]
pub(crate) struct SessionEpoch {
    state: Arc<EpochState>,
}

impl SessionEpoch {
    fn new(id: u64) -> Self {
        Self {
            state: Arc::new(EpochState {
                id,
                cancelled: AtomicBool::new(false),
                changed: Notify::new(),
            }),
        }
    }

    pub(crate) fn id(&self) -> u64 {
        self.state.id
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }

    pub(crate) async fn cancelled(&self) {
        let changed = self.state.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        changed.await;
    }

    fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::Release);
        self.state.changed.notify_waiters();
    }
}

#[derive(Debug)]
enum Admission {
    Open,
    Closed(Arc<CloseIdentity>),
    Shutdown,
}

#[derive(Debug)]
struct CloseIdentity;

#[derive(Debug)]
struct AdmissionState {
    admission: Admission,
    requests: usize,
    jobs: usize,
    epoch: SessionEpoch,
    next_epoch: u64,
}

/// Mutable admission and ownership state inside one complete generation.
#[derive(Debug)]
pub(crate) struct AdmissionGate {
    state: Mutex<AdmissionState>,
    changed: Condvar,
}

impl Default for AdmissionGate {
    fn default() -> Self {
        Self {
            state: Mutex::new(AdmissionState {
                admission: Admission::Open,
                requests: 0,
                jobs: 0,
                epoch: SessionEpoch::new(1),
                next_epoch: 2,
            }),
            changed: Condvar::new(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct CloseToken {
    identity: Arc<CloseIdentity>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum DrainOutcome {
    Idle,
    Invalidated,
    TimedOut,
}

impl AdmissionGate {
    pub(crate) fn admit(self: &Arc<Self>) -> Option<AdmissionLease> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !matches!(state.admission, Admission::Open) {
            return None;
        }
        state.requests += 1;
        let epoch = state.epoch.clone();
        Some(AdmissionLease {
            owner: Arc::new(RequestOwner {
                gate: Arc::clone(self),
            }),
            epoch,
        })
    }

    pub(crate) fn close(&self) -> Option<CloseToken> {
        let (old_epoch, token) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if !matches!(state.admission, Admission::Open) {
                return None;
            }
            let identity = Arc::new(CloseIdentity);
            let next_epoch = SessionEpoch::new(state.next_epoch);
            state.next_epoch = state.next_epoch.wrapping_add(1).max(1);
            let old_epoch = std::mem::replace(&mut state.epoch, next_epoch);
            state.admission = Admission::Closed(Arc::clone(&identity));
            (old_epoch, CloseToken { identity })
        };
        old_epoch.cancel();
        Some(token)
    }

    pub(crate) fn reopen(&self, token: &CloseToken) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let matches = matches!(
            &state.admission,
            Admission::Closed(identity) if Arc::ptr_eq(identity, &token.identity)
        );
        if matches {
            state.admission = Admission::Open;
        }
        drop(state);
        if matches {
            self.changed.notify_all();
        }
        matches
    }

    pub(crate) fn shutdown(&self) {
        let epoch = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.admission = Admission::Shutdown;
            state.epoch.clone()
        };
        epoch.cancel();
        self.changed.notify_all();
    }

    pub(crate) fn is_open(&self) -> bool {
        matches!(
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .admission,
            Admission::Open
        )
    }

    pub(crate) fn counts(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        (state.requests, state.jobs)
    }

    pub(crate) fn wait_for_idle(&self, token: &CloseToken, deadline: Instant) -> DrainOutcome {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if !matches!(
                &state.admission,
                Admission::Closed(identity) if Arc::ptr_eq(identity, &token.identity)
            ) {
                return DrainOutcome::Invalidated;
            }
            if state.requests == 0 && state.jobs == 0 {
                return DrainOutcome::Idle;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return DrainOutcome::TimedOut;
            }
            let (next, timeout) = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
            if timeout.timed_out() && (state.requests != 0 || state.jobs != 0) {
                return DrainOutcome::TimedOut;
            }
        }
    }

    pub(crate) fn wait_until_idle(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.requests != 0 || state.jobs != 0 {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn start_job(self: &Arc<Self>, epoch: &SessionEpoch) -> Option<JobLease> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !matches!(state.admission, Admission::Open)
            || !Arc::ptr_eq(&state.epoch.state, &epoch.state)
            || epoch.is_cancelled()
        {
            return None;
        }
        state.jobs += 1;
        Some(JobLease {
            gate: Some(Arc::clone(self)),
        })
    }

    fn finish_request(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        debug_assert!(state.requests > 0);
        state.requests = state.requests.saturating_sub(1);
        drop(state);
        self.changed.notify_all();
    }

    fn finish_job(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        debug_assert!(state.jobs > 0);
        state.jobs = state.jobs.saturating_sub(1);
        drop(state);
        self.changed.notify_all();
    }
}

#[derive(Debug)]
struct RequestOwner {
    gate: Arc<AdmissionGate>,
}

impl Drop for RequestOwner {
    fn drop(&mut self) {
        self.gate.finish_request();
    }
}

/// Shared ownership for one admitted request or session.
#[derive(Clone, Debug)]
pub(crate) struct AdmissionLease {
    owner: Arc<RequestOwner>,
    epoch: SessionEpoch,
}

impl AdmissionLease {
    pub(crate) fn epoch(&self) -> &SessionEpoch {
        &self.epoch
    }

    pub(crate) fn own_job(&self) -> Option<JobLease> {
        self.owner.gate.start_job(&self.epoch)
    }
}

/// Explicit ownership for one admitted worker job.
#[derive(Debug)]
pub(crate) struct JobLease {
    gate: Option<Arc<AdmissionGate>>,
}

impl Drop for JobLease {
    fn drop(&mut self) {
        if let Some(gate) = self.gate.take() {
            gate.finish_job();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AdmissionGate, DrainOutcome, ReplacementCoordinator};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn miri_admission_counts_requests_and_jobs_without_reference_counts() {
        let gate = Arc::new(AdmissionGate::default());
        let request = gate.admit().expect("open gate admits");
        let job = request.own_job().expect("current epoch admits work");
        assert_eq!(gate.counts(), (1, 1));

        drop(request);
        assert_eq!(gate.counts(), (0, 1));
        drop(job);
        assert_eq!(gate.counts(), (0, 0));
    }

    #[test]
    fn miri_reopen_installs_a_fresh_epoch_and_rejects_stale_work() {
        let gate = Arc::new(AdmissionGate::default());
        let stale = gate.admit().expect("first epoch admits");
        let old_epoch = stale.epoch().id();
        let close = gate.close().expect("open gate closes");
        assert!(stale.epoch().is_cancelled());
        assert!(stale.own_job().is_none());
        drop(stale);
        assert_eq!(
            gate.wait_for_idle(&close, Instant::now() + Duration::from_secs(1)),
            DrainOutcome::Idle
        );
        assert!(gate.reopen(&close));

        let fresh = gate.admit().expect("rollback epoch admits");
        assert_ne!(fresh.epoch().id(), old_epoch);
        assert!(!fresh.epoch().is_cancelled());
    }

    #[test]
    fn miri_shutdown_invalidates_a_staged_replacement_permit() {
        let coordinator = Arc::new(ReplacementCoordinator::default());
        let replacement = coordinator.acquire();
        assert!(replacement.is_current());
        {
            let _shutdown = coordinator.begin_shutdown();
            assert!(!replacement.is_current());
        }
        assert!(!replacement.is_current());
    }
}
