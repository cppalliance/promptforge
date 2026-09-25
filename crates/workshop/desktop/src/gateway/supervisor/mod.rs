//! Continuous local Gateway supervision and recovery.

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use gateway_api_discovery::{CancellationToken, Resolution, SidecarError, ValidatedConnection};

use super::boot;
use super::identity::GatewayAttachment;

mod launch;
mod lifecycle;
mod recovery;
mod signals;

#[cfg(test)]
pub(super) use launch::{
    launch_and_attach_cancellable_with, run_effect_if_active,
    wait_for_launched_file_cancellable_with,
};
#[cfg(test)]
pub(super) use lifecycle::SUPERVISOR_SHUTDOWN_BUDGET;
pub(crate) use lifecycle::{GatewaySupervisor, SupervisorShutdown};
pub(super) use recovery::RecoveryIdentity;
pub(crate) use recovery::{RecoveryCandidate, RecoveryOwnership};

use launch::launch_and_attach_cancellable;

/// Healthy-sidecar supervision cadence.
const SUPERVISION_INTERVAL: Duration = Duration::from_secs(5);

/// First delay after a failed re-resolution or relaunch.
const SUPERVISION_BASE_DELAY: Duration = Duration::from_millis(250);

/// Ceiling on repeated sidecar recovery attempts.
pub(super) const SUPERVISION_MAX_DELAY: Duration = Duration::from_secs(30);

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

    /// Shuts down an unpublished owned child through the explicit,
    /// error-reporting path. Identities that own no child do nothing.
    fn shutdown_unpublished(self)
    where
        Self: Sized,
    {
    }
}

/// Starts runtime supervision only for a gateway discovery file sidecar.
///
/// # Errors
/// Returns an error when the supervisor cannot locate its runtime paths or
/// spawn its owned thread.
pub(crate) fn supervise(
    attachment: &GatewayAttachment,
    updater: workshop_server_api::GatewayUpdater,
) -> anyhow::Result<Option<GatewaySupervisor>> {
    let Some(initial) = attachment.sidecar_identity().cloned() else {
        return Ok(None);
    };
    let run_dir =
        gateway_api_discovery::default_run_dir().context("locate the sidecar run directory")?;
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
            |_, cancellation| match gateway_api_discovery::resolve_cancellable(
                &run_dir,
                cancellation,
            ) {
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
                        // A recovered child this process launched stays
                        // unpublished, so it is shut down through the
                        // explicit, error-reporting path.
                        identity.shutdown_unpublished();
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
