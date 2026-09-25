//! Supervisor thread lifecycle and bounded shutdown.

use std::time::{Duration, Instant};

use anyhow::Context as _;
use gateway_api_discovery::CancellationToken;

use super::signals::{Completion, StopSignal};

/// Maximum designed supervisor shutdown latency.
pub(crate) const SUPERVISOR_SHUTDOWN_BUDGET: Duration = Duration::from_secs(3);

/// The running local-sidecar supervisor.
#[derive(Debug)]
pub(crate) struct GatewaySupervisor {
    stop: StopSignal,
    completion: Completion,
    stop_bridge_completion: Completion,
    thread: Option<std::thread::JoinHandle<()>>,
    stop_bridge: Option<std::thread::JoinHandle<()>>,
    publication: Option<workshop_server_api::GatewayUpdater>,
    shutdown_budget: Duration,
}

/// How bounded supervisor shutdown ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SupervisorShutdown {
    /// The worker completed and joined normally.
    Joined,
    /// The worker completed but panicked.
    Panicked,
    /// The deadline elapsed, so the worker handle was detached.
    Detached,
}

impl GatewaySupervisor {
    /// Spawns one owned supervisor thread.
    #[cfg(test)]
    pub(crate) fn spawn(
        supervise: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> anyhow::Result<Self> {
        Self::spawn_inner(None, SUPERVISOR_SHUTDOWN_BUDGET, supervise)
    }

    pub(crate) fn spawn_with_publication(
        publication: workshop_server_api::GatewayUpdater,
        supervise: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> anyhow::Result<Self> {
        Self::spawn_inner(Some(publication), SUPERVISOR_SHUTDOWN_BUDGET, supervise)
    }

    #[cfg(test)]
    pub(crate) fn spawn_with_budget(
        shutdown_budget: Duration,
        supervise: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> anyhow::Result<Self> {
        Self::spawn_inner(None, shutdown_budget, supervise)
    }

    fn spawn_inner(
        publication: Option<workshop_server_api::GatewayUpdater>,
        shutdown_budget: Duration,
        supervise: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> anyhow::Result<Self> {
        let cancellation = CancellationToken::new();
        let stop = StopSignal::default();
        let worker_stop = stop.clone();
        let completion = Completion::default();
        let worker_completion = completion.clone();
        let stop_bridge_completion = Completion::default();
        let bridge_completion = stop_bridge_completion.clone();
        let bridge_cancellation = cancellation.clone();
        let stop_bridge = std::thread::Builder::new()
            .name("gateway-supervisor-stop".to_owned())
            .spawn(move || {
                let _completion = bridge_completion.guard();
                worker_stop.wait();
                bridge_cancellation.cancel();
            })
            .context("spawn the gateway supervisor stop bridge")?;
        let thread = match std::thread::Builder::new()
            .name("gateway-supervisor".to_owned())
            .spawn(move || {
                let _completion = worker_completion.guard();
                supervise(cancellation);
            }) {
            Ok(thread) => thread,
            Err(source) => {
                stop.signal();
                let error = anyhow::Error::new(source).context("spawn the gateway supervisor");
                return match stop_bridge.join() {
                    Ok(()) => Err(error),
                    Err(_) => Err(error.context(
                        "the gateway supervisor stop bridge panicked during spawn rollback",
                    )),
                };
            }
        };
        Ok(Self {
            stop,
            completion,
            stop_bridge_completion,
            thread: Some(thread),
            stop_bridge: Some(stop_bridge),
            publication,
            shutdown_budget,
        })
    }

    /// Revokes publication, requests stop, and waits at most one deadline.
    ///
    /// This is the blocking, outcome-reporting path; `Drop` only signals
    /// and detaches.
    pub(crate) fn shutdown(mut self) -> SupervisorShutdown {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> SupervisorShutdown {
        let deadline = Instant::now() + self.shutdown_budget;
        if let Some(publication) = self.publication.as_ref() {
            publication.close_publication();
        }
        self.stop.signal();
        let thread = self.thread.take();
        let stop_bridge = self.stop_bridge.take();
        let (Some(thread), Some(stop_bridge)) = (thread, stop_bridge) else {
            return SupervisorShutdown::Joined;
        };
        if !self.completion.wait_until(deadline)
            || !self.stop_bridge_completion.wait_until(deadline)
        {
            drop(thread);
            drop(stop_bridge);
            return SupervisorShutdown::Detached;
        }
        match (thread.join(), stop_bridge.join()) {
            (Ok(()), Ok(())) => SupervisorShutdown::Joined,
            (Err(_), _) | (_, Err(_)) => SupervisorShutdown::Panicked,
        }
    }

    #[cfg(test)]
    pub(crate) fn completion_waker_for_test(&self) -> impl Fn() + Send + 'static {
        let completion = self.completion.clone();
        move || completion.wake()
    }
}

impl Drop for GatewaySupervisor {
    fn drop(&mut self) {
        // `shutdown()` is the bounded, outcome-reporting path. Drop can
        // neither wait nor report, so it revokes publication, signals the
        // stop, and detaches both threads; the worker captures only owned
        // state, so a detached thread finishes on its own.
        if let Some(publication) = self.publication.as_ref() {
            publication.close_publication();
        }
        self.stop.signal();
        drop(self.thread.take());
        drop(self.stop_bridge.take());
    }
}
