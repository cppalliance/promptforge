//! Continuous local Gateway supervision and recovery.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use shared_sidecar::{
    CancellationToken, ConnectionFile, LaunchDecision, Resolution, SidecarError,
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
}

impl SupervisedGatewayIdentity for ValidatedConnection {
    fn same_boot(&self, other: &Self) -> bool {
        same_gateway_identity(self, other)
    }
}

/// The running local-sidecar supervisor.
#[derive(Debug)]
pub(crate) struct GatewaySupervisor {
    cancellation: CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl GatewaySupervisor {
    /// Spawns one owned supervisor thread.
    pub(super) fn spawn(
        supervise: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> anyhow::Result<Self> {
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let thread = std::thread::Builder::new()
            .name("gateway-supervisor".to_owned())
            .spawn(move || supervise(worker_cancellation))
            .context("spawn the gateway supervisor")?;
        Ok(Self {
            cancellation,
            thread: Some(thread),
        })
    }

    /// Cancels supervision and joins its thread.
    pub(crate) fn shutdown(mut self) {
        self.cancel_and_join();
    }

    fn cancel_and_join(&mut self) {
        let started = Instant::now();
        self.cancellation.cancel();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("the gateway supervisor panicked during shutdown");
        }
        let elapsed = started.elapsed();
        if elapsed > SUPERVISOR_SHUTDOWN_BUDGET {
            eprintln!(
                "the gateway supervisor exceeded its {SUPERVISOR_SHUTDOWN_BUDGET:?} shutdown budget: {elapsed:?}"
            );
        }
    }
}

impl Drop for GatewaySupervisor {
    fn drop(&mut self) {
        self.cancel_and_join();
    }
}

/// Starts runtime supervision only for a connection-file sidecar.
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
    GatewaySupervisor::spawn(move |cancellation| {
        run_supervision(
            initial,
            |_, cancellation| match shared_sidecar::resolve_cancellable(&run_dir, cancellation) {
                Ok(Resolution::Attach(file)) => {
                    match ValidatedConnection::validate_cancellable(file, cancellation) {
                        Ok(identity) => SupervisionProbe::Replacement(identity),
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
                let file = launch_and_attach_cancellable(&run_dir, exe, cancellation)?;
                ValidatedConnection::validate_cancellable(file, cancellation)
                    .context("retain the recovered gateway identity")
            },
            |validated, cancellation| {
                if updater
                    .replace_sidecar_cancellable(validated, cancellation)
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
            SupervisionProbe::Replacement(identity) => match publish(&identity, cancellation) {
                Ok(()) => {
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
                Ok(identity) => match publish(&identity, cancellation) {
                    Ok(()) => {
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
) -> anyhow::Result<ConnectionFile> {
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
) -> anyhow::Result<ConnectionFile>
where
    Settle: FnOnce(&Path, Duration, &CancellationToken) -> Result<LaunchDecision, SidecarError>,
    Spawn: FnOnce(&Path, &CancellationToken) -> std::io::Result<()>,
    Wait: FnOnce(&Path, Duration, &CancellationToken) -> anyhow::Result<ConnectionFile>,
{
    match settle(run_dir, RECOVERY_TIMEOUT, cancellation)
        .context("settle the gateway launch race")?
    {
        LaunchDecision::Attach(file) => {
            if cancellation.is_cancelled() {
                anyhow::bail!("gateway attachment was cancelled");
            }
            Ok(file)
        }
        LaunchDecision::Launch(lock) => {
            run_effect_if_active(cancellation, "gateway launch", |cancellation| {
                if cancellation.is_cancelled() {
                    anyhow::bail!("gateway launch was cancelled");
                }
                spawn(exe, cancellation).with_context(|| format!("spawn {}", exe.display()))
            })?;
            let file = wait(run_dir, RECOVERY_TIMEOUT, cancellation)?;
            drop(lock);
            Ok(file)
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
) -> anyhow::Result<ConnectionFile> {
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
) -> anyhow::Result<ConnectionFile>
where
    Health: FnMut(&str, Duration, &CancellationToken) -> Result<(), shared_sidecar::HealthError>,
    Resolve: FnMut(&Path, &CancellationToken) -> Result<Resolution, SidecarError>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if cancellation.is_cancelled() {
            anyhow::bail!("the launched gateway wait was cancelled");
        }
        if let Ok(Some(file)) = ConnectionFile::read(run_dir) {
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
                "the launched gateway wrote no validated connection file within {timeout:?}"
            );
        }
        if cancellation.wait_timeout(RECOVERY_POLL_INTERVAL) {
            anyhow::bail!("the launched gateway wait was cancelled");
        }
    }
}
