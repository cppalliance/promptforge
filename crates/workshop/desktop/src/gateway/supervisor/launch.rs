//! Cancellable recovery launch and readiness wait.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gateway_api_discovery::{
    CancellationToken, GatewayDiscoveryFile, LaunchDecision, Resolution, SidecarError,
};

use super::boot;

/// Budget for recovery launch-race and readiness phases.
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Delay between recovery readiness polls.
const RECOVERY_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Settles and performs a cancellable recovery launch.
pub(super) fn launch_and_attach_cancellable(
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

/// Recovery launch with each blocking phase injected.
pub(crate) fn launch_and_attach_cancellable_with<Settle, Spawn, Wait>(
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
pub(crate) fn run_effect_if_active<T>(
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
        gateway_api_discovery::wait_for_health_cancellable,
        gateway_api_discovery::resolve_cancellable,
    )
}

/// Recovery readiness wait with health and validation injected.
pub(crate) fn wait_for_launched_file_cancellable_with<Health, Resolve>(
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
