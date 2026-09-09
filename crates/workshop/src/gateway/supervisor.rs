//! Continuous local Gateway supervision and recovery.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use shared_sidecar::{
    CancellationToken, GatewayDiscoveryFile, LaunchDecision, Resolution, SidecarError,
    ValidatedConnection,
};

use super::boot;
use super::identity::{GatewayAttachment, same_gateway_identity};

/// Healthy-sidecar supervision cadence.
const SUPERVISION_INTERVAL: Duration = Duration::from_secs(5);

/// First delay after a failed re-resolution or relaunch.
const SUPERVISION_BASE_DELAY: Duration = Duration::from_millis(250);

/// Ceiling on repeated sidecar recovery attempts.
pub(super) const SUPERVISION_MAX_DELAY: Duration = Duration::from_secs(30);

/// Maximum designed supervisor shutdown latency.
const SUPERVISOR_SHUTDOWN_BUDGET: Duration = Duration::from_secs(3);

/// Budget for recovery launch-race and readiness phases.
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Separate bound for authenticated cleanup of an unpublished owned child.
const LATE_CHILD_SHUTDOWN_BUDGET: Duration = Duration::from_secs(1);

/// Delay between recovery readiness polls.
const RECOVERY_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One sidecar liveness observation.
pub(super) enum SupervisionProbe<Identity> {
    /// Another process already published a live replacement.
    Replacement(Identity),
    /// No live local Gateway is currently discoverable.
    Missing,
}

/// A validated identity retained across supervision classifications.
pub(super) trait SupervisedGatewayIdentity {
    /// Whether both values prove the same process boot.
    fn same_boot(&self, other: &Self) -> bool;

    /// Disarms cleanup after this identity becomes authoritative.
    fn publication_succeeded(&mut self) {}
}

/// A validated recovery process whose pid proves it is the child we spawned.
#[derive(Debug)]
pub(crate) struct RecoveryCandidate {
    child_pid: u32,
    validated: ValidatedConnection,
    published: bool,
}

pub(super) enum RecoveryOwnership {
    Owned(RecoveryCandidate),
    Unowned(ValidatedConnection),
}

impl RecoveryCandidate {
    /// Claims cleanup authority only when validation names the spawned pid.
    pub(super) fn authenticate(
        child_pid: u32,
        validated: ValidatedConnection,
    ) -> RecoveryOwnership {
        if validated.pid() != child_pid {
            return RecoveryOwnership::Unowned(validated);
        }
        RecoveryOwnership::Owned(Self {
            child_pid,
            validated,
            published: false,
        })
    }

    pub(super) fn validated(&self) -> &ValidatedConnection {
        &self.validated
    }

    pub(super) fn published(&mut self) {
        self.published = true;
    }
}

impl Drop for RecoveryCandidate {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        debug_assert_eq!(self.child_pid, self.validated.pid());
        let deadline = Instant::now() + LATE_CHILD_SHUTDOWN_BUDGET;
        if let Err(error) = shared_sidecar::request_shutdown_before(&self.validated, deadline) {
            // Drop has no error return channel. The bounded authenticated
            // request is best effort, so diagnostics are the only place this
            // cleanup failure can be surfaced without aborting teardown.
            eprintln!("could not shut down an unpublished recovered gateway: {error}");
        }
    }
}

impl SupervisedGatewayIdentity for ValidatedConnection {
    fn same_boot(&self, other: &Self) -> bool {
        same_gateway_identity(self, other)
    }
}

#[derive(Debug)]
pub(super) enum RecoveryIdentity {
    Stable(ValidatedConnection),
    Candidate(RecoveryCandidate),
}

impl RecoveryIdentity {
    fn validated(&self) -> &ValidatedConnection {
        match self {
            Self::Stable(validated) => validated,
            Self::Candidate(candidate) => candidate.validated(),
        }
    }
}

impl SupervisedGatewayIdentity for RecoveryIdentity {
    fn same_boot(&self, other: &Self) -> bool {
        self.validated().same_boot(other.validated())
    }

    fn publication_succeeded(&mut self) {
        if let Self::Candidate(candidate) = self {
            candidate.published();
        }
    }
}

/// The running local-sidecar supervisor.
#[derive(Debug)]
pub(crate) struct GatewaySupervisor {
    stop: StopSignal,
    completion: Completion,
    stop_bridge_completion: Completion,
    thread: Option<std::thread::JoinHandle<()>>,
    stop_bridge: Option<std::thread::JoinHandle<()>>,
    publication: Option<workshop_server::GatewayUpdater>,
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

/// A stop request that never waits for in-progress supervisor work.
#[derive(Clone, Debug, Default)]
struct StopSignal {
    state: Arc<StopState>,
}

#[derive(Debug, Default)]
struct StopState {
    requested: AtomicBool,
    waiter: Mutex<()>,
    wake: Condvar,
}

impl StopSignal {
    fn signal(&self) {
        let waiter = self
            .state
            .waiter
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.state.requested.store(true, Ordering::SeqCst);
        self.state.wake.notify_all();
        drop(waiter);
    }

    fn wait(&self) {
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
struct Completion {
    state: Arc<CompletionState>,
}

#[derive(Debug, Default)]
struct CompletionState {
    finished: Mutex<bool>,
    wake: Condvar,
}

impl Completion {
    fn guard(&self) -> CompletionGuard {
        CompletionGuard(self.clone())
    }

    fn wait_until(&self, deadline: Instant) -> bool {
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
    fn wake(&self) {
        self.state.wake.notify_all();
    }
}

struct CompletionGuard(Completion);

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

impl GatewaySupervisor {
    /// Spawns one owned supervisor thread.
    #[cfg(test)]
    pub(super) fn spawn(
        supervise: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> anyhow::Result<Self> {
        Self::spawn_inner(None, SUPERVISOR_SHUTDOWN_BUDGET, supervise)
    }

    pub(super) fn spawn_with_publication(
        publication: workshop_server::GatewayUpdater,
        supervise: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> anyhow::Result<Self> {
        Self::spawn_inner(Some(publication), SUPERVISOR_SHUTDOWN_BUDGET, supervise)
    }

    #[cfg(test)]
    pub(super) fn spawn_with_budget(
        shutdown_budget: Duration,
        supervise: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> anyhow::Result<Self> {
        Self::spawn_inner(None, shutdown_budget, supervise)
    }

    fn spawn_inner(
        publication: Option<workshop_server::GatewayUpdater>,
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
    pub(super) fn wake_completion_for_test(&self) {
        self.completion.wake();
    }
}

impl Drop for GatewaySupervisor {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

/// Starts runtime supervision only for a gateway discovery file sidecar.
///
/// # Errors
/// Returns an error when the supervisor cannot locate its runtime paths or
/// spawn its owned thread.
pub(crate) fn supervise(
    attachment: &GatewayAttachment,
    updater: workshop_server::GatewayUpdater,
) -> anyhow::Result<Option<GatewaySupervisor>> {
    let Some(initial) = attachment.sidecar_identity().cloned() else {
        return Ok(None);
    };
    let run_dir = shared_sidecar::default_run_dir().context("locate the sidecar run directory")?;
    let exe_dir = std::env::current_exe()
        .context("locate the executable")?
        .parent()
        .map(Path::to_path_buf)
        .context("the executable has no parent directory")?;
    let sibling = boot::sibling_gateway(&exe_dir);
    let supervisor_publication = updater.clone();
    GatewaySupervisor::spawn_with_publication(supervisor_publication, move |cancellation| {
        run_supervision(
            RecoveryIdentity::Stable(initial),
            |_, cancellation| match shared_sidecar::resolve_cancellable(&run_dir, cancellation) {
                Ok(Resolution::Attach(file)) => {
                    match ValidatedConnection::validate_cancellable(file, cancellation) {
                        Ok(identity) => {
                            SupervisionProbe::Replacement(RecoveryIdentity::Stable(identity))
                        }
                        Err(error) => {
                            eprintln!("could not retain the replacement gateway identity: {error}");
                            SupervisionProbe::Missing
                        }
                    }
                }
                Ok(_) | Err(SidecarError::Cancelled) => SupervisionProbe::Missing,
                Err(error) => {
                    eprintln!("could not re-resolve the local gateway: {error}");
                    SupervisionProbe::Missing
                }
            },
            |cancellation| {
                let exe = sibling.as_deref().context(
                    "the local gateway disappeared and no sibling gateway executable is installed",
                )?;
                let recovery = launch_and_attach_cancellable(&run_dir, exe, cancellation)?;
                validate_recovery(recovery, cancellation)
            },
            |identity, cancellation| {
                if cancellation.is_cancelled() {
                    anyhow::bail!("gateway publication was cancelled");
                }
                if updater.publication_closed() {
                    anyhow::bail!("gateway publication is closed");
                }
                if updater
                    .replace_sidecar_cancellable(identity.validated(), cancellation)
                    .context("publish the replacement gateway endpoint")?
                {
                    Ok(())
                } else {
                    anyhow::bail!("gateway publication was cancelled")
                }
            },
            |delay, cancellation| cancellation.wait_timeout(delay),
            &cancellation,
        );
    })
    .map(Some)
}

/// Validates a recovery result and authenticates child ownership by exact pid.
pub(super) fn validate_recovery(
    recovery: boot::RecoveryLaunch,
    cancellation: &CancellationToken,
) -> anyhow::Result<RecoveryIdentity> {
    let (child_pid, file) = match recovery {
        boot::RecoveryLaunch::Attached(file) => (None, file),
        boot::RecoveryLaunch::Launched { child_pid, file } => (Some(child_pid), file),
    };
    let validated = ValidatedConnection::validate_cancellable(file, cancellation)
        .context("retain the recovered gateway identity")?;
    Ok(match child_pid {
        Some(child_pid) => match RecoveryCandidate::authenticate(child_pid, validated) {
            RecoveryOwnership::Owned(candidate) => RecoveryIdentity::Candidate(candidate),
            RecoveryOwnership::Unowned(unowned) => RecoveryIdentity::Stable(unowned),
        },
        None => RecoveryIdentity::Stable(validated),
    })
}

/// Runs the supervision state machine with I/O injected for tests.
pub(super) fn run_supervision<Identity, Probe, Recover, Publish, Wait, Error>(
    mut current: Identity,
    mut probe: Probe,
    mut recover: Recover,
    mut publish: Publish,
    mut wait: Wait,
    cancellation: &CancellationToken,
) where
    Identity: SupervisedGatewayIdentity,
    Probe: FnMut(&Identity, &CancellationToken) -> SupervisionProbe<Identity>,
    Recover: FnMut(&CancellationToken) -> Result<Identity, Error>,
    Publish: FnMut(&Identity, &CancellationToken) -> Result<(), Error>,
    Wait: FnMut(Duration, &CancellationToken) -> bool,
    Error: std::fmt::Display,
{
    let mut retry_delay = SUPERVISION_BASE_DELAY;
    loop {
        if cancellation.is_cancelled() {
            return;
        }
        let observation = probe(&current, cancellation);
        if cancellation.is_cancelled() {
            return;
        }
        match observation {
            SupervisionProbe::Replacement(identity) if identity.same_boot(&current) => {
                retry_delay = SUPERVISION_BASE_DELAY;
                if wait(SUPERVISION_INTERVAL, cancellation) {
                    return;
                }
                continue;
            }
            SupervisionProbe::Replacement(mut identity) => match publish(&identity, cancellation) {
                Ok(()) => {
                    identity.publication_succeeded();
                    if cancellation.is_cancelled() {
                        return;
                    }
                    current = identity;
                    retry_delay = SUPERVISION_BASE_DELAY;
                    continue;
                }
                Err(error) => {
                    eprintln!("could not publish a replacement local gateway: {error}");
                }
            },
            SupervisionProbe::Missing => match recover(cancellation) {
                Ok(_) if cancellation.is_cancelled() => return,
                Ok(mut identity) => match publish(&identity, cancellation) {
                    Ok(()) => {
                        identity.publication_succeeded();
                        if cancellation.is_cancelled() {
                            return;
                        }
                        current = identity;
                        retry_delay = SUPERVISION_BASE_DELAY;
                        continue;
                    }
                    Err(error) => {
                        eprintln!("could not publish a replacement local gateway: {error}");
                    }
                },
                Err(error) => {
                    eprintln!("could not recover the local gateway: {error}");
                }
            },
        }
        if cancellation.is_cancelled() || wait(retry_delay, cancellation) {
            return;
        }
        retry_delay = retry_delay.saturating_mul(2).min(SUPERVISION_MAX_DELAY);
    }
}

/// Settles and performs a cancellable recovery launch.
fn launch_and_attach_cancellable(
    run_dir: &Path,
    exe: &Path,
    cancellation: &CancellationToken,
) -> anyhow::Result<boot::RecoveryLaunch> {
    launch_and_attach_cancellable_with(
        run_dir,
        exe,
        cancellation,
        shared_sidecar::launch_or_attach_cancellable,
        |exe, _| boot::spawn_detached(exe),
        wait_for_launched_file_cancellable,
    )
}

/// Recovery launch with each blocking phase injected.
pub(super) fn launch_and_attach_cancellable_with<Settle, Spawn, Wait>(
    run_dir: &Path,
    exe: &Path,
    cancellation: &CancellationToken,
    settle: Settle,
    spawn: Spawn,
    wait: Wait,
) -> anyhow::Result<boot::RecoveryLaunch>
where
    Settle: FnOnce(&Path, Duration, &CancellationToken) -> Result<LaunchDecision, SidecarError>,
    Spawn: FnOnce(&Path, &CancellationToken) -> std::io::Result<u32>,
    Wait: FnOnce(&Path, Duration, &CancellationToken) -> anyhow::Result<GatewayDiscoveryFile>,
{
    match settle(run_dir, RECOVERY_TIMEOUT, cancellation)
        .context("settle the gateway launch race")?
    {
        LaunchDecision::Attach(file) => {
            if cancellation.is_cancelled() {
                anyhow::bail!("gateway attachment was cancelled");
            }
            Ok(boot::RecoveryLaunch::Attached(file))
        }
        LaunchDecision::Launch(lock) => {
            let child_pid = run_effect_if_active(cancellation, "gateway launch", |cancellation| {
                if cancellation.is_cancelled() {
                    anyhow::bail!("gateway launch was cancelled");
                }
                spawn(exe, cancellation).with_context(|| format!("spawn {}", exe.display()))
            })?;
            let file = wait(run_dir, RECOVERY_TIMEOUT, cancellation)?;
            drop(lock);
            Ok(boot::RecoveryLaunch::Launched { child_pid, file })
        }
        decision => anyhow::bail!("an unknown launch decision: {decision:?}"),
    }
}

/// Linearizes one externally visible recovery effect with cancellation.
pub(super) fn run_effect_if_active<T>(
    cancellation: &CancellationToken,
    phase: &'static str,
    operation: impl FnOnce(&CancellationToken) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    match cancellation.run_if_active(|| operation(cancellation)) {
        Some(result) => result,
        None => anyhow::bail!("{phase} was cancelled"),
    }
}

/// Waits for a launched recovery Gateway with production probes.
fn wait_for_launched_file_cancellable(
    run_dir: &Path,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> anyhow::Result<GatewayDiscoveryFile> {
    wait_for_launched_file_cancellable_with(
        run_dir,
        timeout,
        cancellation,
        shared_sidecar::wait_for_health_cancellable,
        shared_sidecar::resolve_cancellable,
    )
}

/// Recovery readiness wait with health and validation injected.
pub(super) fn wait_for_launched_file_cancellable_with<Health, Resolve>(
    run_dir: &Path,
    timeout: Duration,
    cancellation: &CancellationToken,
    mut health: Health,
    mut resolve: Resolve,
) -> anyhow::Result<GatewayDiscoveryFile>
where
    Health: FnMut(&str, Duration, &CancellationToken) -> Result<(), shared_sidecar::HealthError>,
    Resolve: FnMut(&Path, &CancellationToken) -> Result<Resolution, SidecarError>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if cancellation.is_cancelled() {
            anyhow::bail!("the launched gateway wait was cancelled");
        }
        if let Ok(Some(file)) = GatewayDiscoveryFile::read(run_dir) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let url = format!("http://127.0.0.1:{}", file.port);
            health(&url, remaining, cancellation)
                .context("the launched gateway did not answer its health probe")?;
            if cancellation.is_cancelled() {
                anyhow::bail!("the launched gateway wait was cancelled");
            }
            match resolve(run_dir, cancellation) {
                Ok(Resolution::Attach(validated)) => {
                    if cancellation.is_cancelled() {
                        anyhow::bail!("the launched gateway wait was cancelled");
                    }
                    return Ok(validated);
                }
                Err(SidecarError::Cancelled) => {
                    anyhow::bail!("the launched gateway wait was cancelled");
                }
                Ok(_) | Err(_) => {}
            }
        }
        if cancellation.is_cancelled() {
            anyhow::bail!("the launched gateway wait was cancelled");
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "the launched gateway wrote no validated gateway discovery file within {timeout:?}"
            );
        }
        if cancellation.wait_timeout(RECOVERY_POLL_INTERVAL) {
            anyhow::bail!("the launched gateway wait was cancelled");
        }
    }
}
