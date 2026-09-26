//! Cancellable launch and readiness wait, shared by boot and recovery.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gateway_api_discovery::{
    CancellationToken, GatewayDiscoveryFile, LaunchDecision, Resolution, SidecarError,
};

use super::boot;

/// Budget for the launch-race and readiness phases.
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Delay between readiness polls.
const RECOVERY_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Longest single health probe, so a discovery file rewritten while a
/// probe is failing is read again well before the readiness budget ends.
const HEALTH_PROBE_WINDOW: Duration = Duration::from_millis(250);

/// Settles and performs a cancellable launch. Boot passes a token that is
/// never cancelled.
pub(in crate::gateway) fn launch_and_attach_cancellable(
    run_dir: &Path,
    exe: &Path,
    cancellation: &CancellationToken,
) -> anyhow::Result<boot::RecoveryLaunch> {
    launch_and_attach_cancellable_with(
        run_dir,
        exe,
        cancellation,
        gateway_api_discovery::launch_or_attach_cancellable,
        |exe, _| boot::spawn_detached(exe),
        wait_for_launched_file_cancellable,
    )
}

/// Launch with each blocking phase injected.
pub(in crate::gateway) fn launch_and_attach_cancellable_with<Settle, Spawn, Wait>(
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
pub(in crate::gateway) fn run_effect_if_active<T>(
    cancellation: &CancellationToken,
    phase: &'static str,
    operation: impl FnOnce(&CancellationToken) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    match cancellation.run_if_active(|| operation(cancellation)) {
        Some(result) => result,
        None => anyhow::bail!("{phase} was cancelled"),
    }
}

/// Waits for a launched Gateway with production probes.
fn wait_for_launched_file_cancellable(
    run_dir: &Path,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> anyhow::Result<GatewayDiscoveryFile> {
    wait_for_launched_file_cancellable_with(
        run_dir,
        timeout,
        cancellation,
        gateway_api_discovery::wait_for_health_cancellable,
        gateway_api_discovery::resolve_cancellable,
    )
}

/// Readiness wait with health and validation injected. A failed probe
/// does not end the wait; the budget does, reporting the last probe error.
pub(in crate::gateway) fn wait_for_launched_file_cancellable_with<Health, Resolve>(
    run_dir: &Path,
    timeout: Duration,
    cancellation: &CancellationToken,
    mut health: Health,
    mut resolve: Resolve,
) -> anyhow::Result<GatewayDiscoveryFile>
where
    Health:
        FnMut(&str, Duration, &CancellationToken) -> Result<(), gateway_api_discovery::HealthError>,
    Resolve: FnMut(&Path, &CancellationToken) -> Result<Resolution, SidecarError>,
{
    let deadline = Instant::now() + timeout;
    let mut last_probe_error = None;
    loop {
        if cancellation.is_cancelled() {
            anyhow::bail!("the launched gateway wait was cancelled");
        }
        if let Ok(Some(file)) = GatewayDiscoveryFile::read(run_dir) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let url = format!("http://127.0.0.1:{}", file.port);
            match health(&url, remaining.min(HEALTH_PROBE_WINDOW), cancellation) {
                Ok(()) => {
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
                        Ok(Resolution::Stale(reason)) => {
                            last_probe_error =
                                Some(anyhow::Error::new(reason).context(
                                    "the launched gateway discovery file failed validation",
                                ));
                        }
                        Err(error) => {
                            last_probe_error = Some(
                                anyhow::Error::new(error)
                                    .context("resolve the launched gateway discovery file"),
                            );
                        }
                        Ok(_) => {}
                    }
                }
                Err(gateway_api_discovery::HealthError::Cancelled) => {
                    anyhow::bail!("the launched gateway wait was cancelled");
                }
                Err(error) => {
                    last_probe_error = Some(
                        anyhow::Error::new(error)
                            .context("the launched gateway did not answer its health probe"),
                    );
                }
            }
        }
        if cancellation.is_cancelled() {
            anyhow::bail!("the launched gateway wait was cancelled");
        }
        if Instant::now() >= deadline {
            let failure = format!(
                "the launched gateway wrote no validated gateway discovery file within {timeout:?}"
            );
            return Err(match last_probe_error {
                Some(error) => error.context(failure),
                None => anyhow::anyhow!(failure),
            });
        }
        if cancellation.wait_timeout(RECOVERY_POLL_INTERVAL) {
            anyhow::bail!("the launched gateway wait was cancelled");
        }
    }
}
