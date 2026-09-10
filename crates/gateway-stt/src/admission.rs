//! Admission, session epochs, and explicit work ownership for the single
//! speech runtime.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use tokio::sync::Notify;

#[derive(Debug)]
struct EpochState {
    cancelled: AtomicBool,
    changed: Notify,
}

/// One cancellation epoch shared by admitted session work. The epoch cancels
/// only when the runtime's admission shuts down.
#[derive(Clone, Debug)]
pub(crate) struct SessionEpoch {
    state: Arc<EpochState>,
}

impl SessionEpoch {
    fn new() -> Self {
        Self {
            state: Arc::new(EpochState {
                cancelled: AtomicBool::new(false),
                changed: Notify::new(),
            }),
        }
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
    Shutdown,
}

#[derive(Debug)]
struct AdmissionState {
    admission: Admission,
    requests: usize,
    jobs: usize,
    epoch: SessionEpoch,
}

/// Mutable admission and ownership state inside the published runtime.
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
                epoch: SessionEpoch::new(),
            }),
            changed: Condvar::new(),
        }
    }
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

    /// Stops admitting work and cancels the session epoch, so admitted
    /// requests settle instead of hanging on a runtime that is going away.
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

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn counts(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        (state.requests, state.jobs)
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
    use super::AdmissionGate;
    use std::sync::Arc;

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
    fn miri_shutdown_cancels_the_epoch_and_stops_admission() {
        let gate = Arc::new(AdmissionGate::default());
        let request = gate.admit().expect("open gate admits");
        let epoch = request.epoch().clone();

        gate.shutdown();

        assert!(epoch.is_cancelled(), "shutdown cancels the session epoch");
        assert!(!gate.is_open());
        assert!(gate.admit().is_none(), "a shut-down gate admits nothing");
        assert!(
            request.own_job().is_none(),
            "a cancelled epoch owns no new work"
        );
        drop(request);
        gate.wait_until_idle();
    }
}
