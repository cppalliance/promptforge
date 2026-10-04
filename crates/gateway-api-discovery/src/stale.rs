//! Stale detection: decide whether a gateway discovery file names a live
//! gateway, and remove it when it does not.
//!
//! A file is live when one OS process boot with a `promptforge-gateway`
//! image is unchanged across a same-socket health and bearer proof, and
//! the file includes a boot identity. Anything else is stale - the Jupyter
//! phantom-server bug class - and the file is deleted so the next reader
//! relaunches instead of retrying a corpse.

use std::fs;
use std::io;
use std::path::Path;

use crate::error::SidecarError;
use crate::paths::gateway_discovery_file_path;
pub(crate) use crate::validated::GATEWAY_IMAGE_NAME;
use crate::validated::{ValidatedConnection, ValidationError};
use crate::{CancellationToken, GatewayDiscoveryFile};

/// What [`resolve`] found in the run directory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Resolution {
    /// A live gateway: attach with these parameters.
    Attach(GatewayDiscoveryFile),
    /// No gateway discovery file exists: nothing to attach to, nothing to clean.
    Absent,
    /// A gateway discovery file existed but was stale; it was removed.
    Stale(StaleReason),
}

/// Why a gateway discovery file was judged stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StaleReason {
    /// The file was not valid JSON or failed validation.
    #[error("the gateway discovery file is invalid")]
    Invalid,
    /// The pid is dead.
    #[error("the recorded gateway process is dead")]
    ProcessDead,
    /// The pid is alive but its image is not a `promptforge-gateway`
    /// binary (a reused pid).
    #[error("the recorded pid belongs to another process image")]
    ImageMismatch,
    /// The gateway discovery file has no usable boot identity.
    #[error("the gateway discovery file has no usable boot identity")]
    BootIdentityInvalid,
    /// The pid changed process boot while validation was in progress.
    #[error("the recorded process identity changed during validation")]
    ProcessChanged,
    /// The health endpoint did not answer 200.
    #[error("the recorded gateway does not answer its health probe")]
    HealthFailed,
    /// The bearer key was rejected.
    #[error("the gateway discovery file bearer was rejected")]
    KeyRejected,
}

/// Resolves the gateway discovery file in `run_dir`: attach parameters for a
/// live gateway, or stale-file cleanup plus the reason.
///
/// # Errors
/// Returns [`SidecarError::Read`] when the file exists but cannot be
/// read, and [`SidecarError::Remove`] when a stale file cannot be
/// deleted.
pub fn resolve(run_dir: &Path) -> Result<Resolution, SidecarError> {
    resolve_named(run_dir, GATEWAY_IMAGE_NAME)
}

/// Resolves the gateway discovery file while observing caller cancellation.
///
/// Cancellation never classifies or deletes the current file.
///
/// # Errors
/// Returns [`SidecarError::Cancelled`] when cancellation wins, plus the
/// read and remove failures documented by [`resolve`].
pub fn resolve_cancellable(
    run_dir: &Path,
    cancellation: &CancellationToken,
) -> Result<Resolution, SidecarError> {
    resolve_named_cancellable(run_dir, GATEWAY_IMAGE_NAME, cancellation)
}

/// [`resolve`] against a caller-named process image, so a consumer's test
/// binary - never named `promptforge-gateway` - can run the full liveness
/// gauntlet. Test builds only, behind the `test-fixtures` feature.
#[cfg(feature = "test-fixtures")]
#[doc(hidden)]
pub fn resolve_for_test(run_dir: &Path, image_name: &str) -> Result<Resolution, SidecarError> {
    resolve_named(run_dir, image_name)
}

/// [`resolve`] against a caller-named process image, so tests can run the
/// full liveness gauntlet from a test binary, which is never named
/// `promptforge-gateway`.
pub(crate) fn resolve_named(run_dir: &Path, image_name: &str) -> Result<Resolution, SidecarError> {
    let file = match GatewayDiscoveryFile::read(run_dir) {
        Ok(Some(file)) => file,
        Ok(None) => return Ok(Resolution::Absent),
        Err(SidecarError::Parse { .. } | SidecarError::Invalid { .. }) => {
            remove_stale(run_dir)?;
            return Ok(Resolution::Stale(StaleReason::Invalid));
        }
        Err(error) => return Err(error),
    };
    match ValidatedConnection::validate_named(file, image_name) {
        Ok(validated) => Ok(Resolution::Attach(validated.into_gateway_discovery_file())),
        Err(reason) => {
            remove_stale(run_dir)?;
            Ok(Resolution::Stale(reason))
        }
    }
}

pub(crate) fn resolve_named_cancellable(
    run_dir: &Path,
    image_name: &str,
    cancellation: &CancellationToken,
) -> Result<Resolution, SidecarError> {
    resolve_named_cancellable_with(
        run_dir,
        image_name,
        cancellation,
        ValidatedConnection::validate_named_cancellable,
    )
}

fn resolve_named_cancellable_with(
    run_dir: &Path,
    image_name: &str,
    cancellation: &CancellationToken,
    validate: impl FnOnce(
        GatewayDiscoveryFile,
        &str,
        &CancellationToken,
    ) -> Result<ValidatedConnection, ValidationError>,
) -> Result<Resolution, SidecarError> {
    resolve_named_cancellable_with_effects(
        run_dir,
        image_name,
        cancellation,
        validate,
        || {},
        remove_stale,
    )
}

fn resolve_named_cancellable_with_effects(
    run_dir: &Path,
    image_name: &str,
    cancellation: &CancellationToken,
    validate: impl FnOnce(
        GatewayDiscoveryFile,
        &str,
        &CancellationToken,
    ) -> Result<ValidatedConnection, ValidationError>,
    mut before_remove: impl FnMut(),
    mut remove: impl FnMut(&Path) -> Result<(), SidecarError>,
) -> Result<Resolution, SidecarError> {
    if cancellation.is_cancelled() {
        return Err(SidecarError::Cancelled);
    }
    let file = match GatewayDiscoveryFile::read(run_dir) {
        Ok(Some(file)) => file,
        Ok(None) if cancellation.is_cancelled() => return Err(SidecarError::Cancelled),
        Ok(None) => return Ok(Resolution::Absent),
        Err(SidecarError::Parse { .. } | SidecarError::Invalid { .. }) => {
            before_remove();
            remove_stale_if_active(run_dir, cancellation, &mut remove)?;
            return Ok(Resolution::Stale(StaleReason::Invalid));
        }
        Err(error) => return Err(error),
    };
    match validate(file, image_name, cancellation) {
        Ok(validated) => Ok(Resolution::Attach(validated.into_gateway_discovery_file())),
        Err(ValidationError::Cancelled) => Err(SidecarError::Cancelled),
        Err(ValidationError::Stale(reason)) => {
            before_remove();
            remove_stale_if_active(run_dir, cancellation, &mut remove)?;
            Ok(Resolution::Stale(reason))
        }
    }
}

fn remove_stale_if_active(
    run_dir: &Path,
    cancellation: &CancellationToken,
    remove: &mut impl FnMut(&Path) -> Result<(), SidecarError>,
) -> Result<(), SidecarError> {
    cancellation
        .run_if_active(|| remove(run_dir))
        .unwrap_or(Err(SidecarError::Cancelled))
}

/// Whether the gateway discovery file in `run_dir` names a live gateway right
/// now, with no cleanup: the read-only check a diagnostics report runs.
/// Stale-file deletion is the prospective owner's privilege, so a stale
/// file reads as not-running and stays on disk for the next launch to
/// clean.
#[must_use]
pub fn is_running(run_dir: &Path) -> bool {
    is_running_named(run_dir, GATEWAY_IMAGE_NAME)
}

/// [`is_running`] against a caller-named process image, so a test binary -
/// never named `promptforge-gateway` - can run the full liveness gauntlet.
pub(crate) fn is_running_named(run_dir: &Path, image_name: &str) -> bool {
    match GatewayDiscoveryFile::read(run_dir) {
        Ok(Some(file)) => is_live(&file, image_name),
        // A missing, unreadable, or invalid file reads as not-running.
        Ok(None) | Err(_) => false,
    }
}

/// Whether the file's gateway is live right now, with no cleanup: the
/// check a launch-race loser runs, since deleting is the lock holder's
/// privilege.
pub(crate) fn is_live(file: &GatewayDiscoveryFile, image_name: &str) -> bool {
    ValidatedConnection::validate_named(file.clone(), image_name).is_ok()
}

pub(crate) fn is_live_cancellable(
    file: &GatewayDiscoveryFile,
    image_name: &str,
    cancellation: &CancellationToken,
) -> Result<bool, SidecarError> {
    match ValidatedConnection::validate_named_cancellable(file.clone(), image_name, cancellation) {
        Ok(_) => Ok(true),
        Err(ValidationError::Stale(_)) => Ok(false),
        Err(ValidationError::Cancelled) => Err(SidecarError::Cancelled),
    }
}

/// Deletes the stale gateway discovery file, tolerating a concurrent deletion.
fn remove_stale(run_dir: &Path) -> Result<(), SidecarError> {
    let path = gateway_discovery_file_path(run_dir);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SidecarError::Remove {
            path,
            source: error,
        }),
    }
}

#[cfg(test)]
#[path = "stale-tests.rs"]
mod tests;
