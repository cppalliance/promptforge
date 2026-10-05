//! A live Gateway connection whose process and authority have been
//! validated.

use std::ffi::OsStr;
use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::health::{self, ConnectionProbe};
use crate::stale::StaleReason;
use crate::sys::{ProcessIdentity, process_identity};
use crate::{CancellationToken, GatewayDiscoveryFile};

/// The image file name a live Gateway process must have.
#[cfg(windows)]
pub(crate) const GATEWAY_IMAGE_NAME: &str = "promptforge-gateway.exe";
/// The image file name a live Gateway process must have.
#[cfg(not(windows))]
pub(crate) const GATEWAY_IMAGE_NAME: &str = "promptforge-gateway";

/// The bearer-gated route used to prove the presented key is accepted.
const KEY_PROBE_PATH: &str = "/v1/models";

/// Budget for proving health without rejecting one transient failure.
const LIVENESS_BUDGET: Duration = Duration::from_secs(2);

/// A cancellable validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ValidationError {
    /// The caller cancelled validation.
    #[error("connection validation was cancelled")]
    Cancelled,
    /// The connection failed one of the liveness or authority checks.
    #[error(transparent)]
    Stale(#[from] StaleReason),
}

/// A Gateway connection proven live and authorized at construction time.
///
/// Safe code outside this crate cannot construct the capability directly.
/// Construction succeeds only after checking the process image, boot
/// identity, health endpoint, and bearer acceptance.
///
/// Validation observes the OS process boot immediately before and after
/// the one TCP connection that makes both network checks. This closes the
/// health-to-bearer replacement gap and rejects pid reuse during that
/// interval. The capability is a point-in-time proof and makes no claim
/// that the process remains live after validation returns.
///
/// The bearer is deliberately absent from [`Debug`](fmt::Debug) output.
///
/// No test-fixture feature exposes another production-capability
/// constructor.
///
/// The crate-private named validator is equally unavailable.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidatedConnection {
    connection: GatewayDiscoveryFile,
    process_identity: ProcessIdentity,
}

impl ValidatedConnection {
    /// Validates a raw gateway discovery file against the production Gateway image.
    ///
    /// # Errors
    /// Returns the first [`StaleReason`] that prevents the raw connection
    /// from proving a live, authorized Gateway boot.
    pub fn validate(connection: GatewayDiscoveryFile) -> Result<Self, StaleReason> {
        Self::validate_named(connection, GATEWAY_IMAGE_NAME)
    }

    /// Validates a raw connection against the production Gateway image
    /// without allowing the network proof to outlive `deadline`.
    ///
    /// # Errors
    /// Returns the first [`StaleReason`] that prevents the raw connection
    /// from proving a live, authorized Gateway boot before `deadline`.
    pub fn validate_before(
        connection: GatewayDiscoveryFile,
        deadline: Instant,
    ) -> Result<Self, StaleReason> {
        Self::validate_named_before(connection, GATEWAY_IMAGE_NAME, deadline)
    }

    /// Validates a raw connection while observing caller cancellation.
    ///
    /// # Errors
    /// Returns [`ValidationError::Cancelled`] when cancellation wins, or
    /// [`ValidationError::Stale`] when a liveness or authority check fails.
    pub fn validate_cancellable(
        connection: GatewayDiscoveryFile,
        cancellation: &CancellationToken,
    ) -> Result<Self, ValidationError> {
        Self::validate_named_cancellable(connection, GATEWAY_IMAGE_NAME, cancellation)
    }

    pub(crate) fn validate_named(
        connection: GatewayDiscoveryFile,
        image_name: &str,
    ) -> Result<Self, StaleReason> {
        validate_with(
            connection,
            image_name,
            process_identity,
            |address, bearer, deadline| {
                health::probe_connection_until(address, KEY_PROBE_PATH, bearer, deadline)
            },
        )
    }

    fn validate_named_before(
        connection: GatewayDiscoveryFile,
        image_name: &str,
        deadline: Instant,
    ) -> Result<Self, StaleReason> {
        validate_before_with(
            connection,
            image_name,
            deadline,
            process_identity,
            |address, bearer, deadline| {
                health::probe_connection_until(address, KEY_PROBE_PATH, bearer, deadline)
            },
        )
    }

    pub(crate) fn validate_named_cancellable(
        connection: GatewayDiscoveryFile,
        image_name: &str,
        cancellation: &CancellationToken,
    ) -> Result<Self, ValidationError> {
        validate_cancellable_with(
            connection,
            image_name,
            cancellation,
            process_identity,
            |address, bearer, budget, cancellation| {
                health::probe_connection_cancellable(
                    address,
                    KEY_PROBE_PATH,
                    bearer,
                    budget,
                    cancellation,
                )
            },
        )
    }

    /// The validated Gateway's loopback port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.connection.port
    }

    /// The validated Gateway process identifier.
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.connection.pid
    }

    /// The validated Gateway boot epoch.
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.connection.epoch
    }

    /// The validated Gateway version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.connection.version
    }

    /// The validated Gateway boot timestamp.
    #[must_use]
    pub fn started_at(&self) -> &str {
        &self.connection.started_at
    }

    /// Whether both capabilities name the same validated Gateway boot.
    #[must_use]
    pub fn same_boot(&self, other: &Self) -> bool {
        self.process_identity == other.process_identity
            && self.pid() == other.pid()
            && self.epoch() == other.epoch()
            && self.started_at() == other.started_at()
    }

    /// The validated bearer required to build an authorized consumer.
    ///
    /// Callers must keep this value out of diagnostics. The capability's
    /// own [`Debug`](fmt::Debug) implementation always redacts it.
    #[doc(hidden)]
    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.connection.api_key
    }

    pub(crate) fn gateway_discovery_file(&self) -> &GatewayDiscoveryFile {
        &self.connection
    }

    pub(crate) fn into_gateway_discovery_file(self) -> GatewayDiscoveryFile {
        self.connection
    }
}

impl fmt::Debug for ValidatedConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedConnection")
            .field("port", &self.port())
            .field("pid", &self.pid())
            .field("epoch", &self.epoch())
            .field("version", &"[REDACTED]")
            .field("started_at", &"[REDACTED]")
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

/// Performs one validation while allowing deterministic observation and
/// network seams in unit tests. Production passes only the platform process
/// observer and the private shared probe.
fn validate_with(
    connection: GatewayDiscoveryFile,
    image_name: &str,
    observe_process: impl FnMut(u32) -> Option<ProcessIdentity>,
    prove_connection: impl FnOnce(&str, &str, Instant) -> ConnectionProbe,
) -> Result<ValidatedConnection, StaleReason> {
    validate_before_with(
        connection,
        image_name,
        Instant::now() + LIVENESS_BUDGET,
        observe_process,
        prove_connection,
    )
}

fn validate_before_with(
    connection: GatewayDiscoveryFile,
    image_name: &str,
    deadline: Instant,
    mut observe_process: impl FnMut(u32) -> Option<ProcessIdentity>,
    prove_connection: impl FnOnce(&str, &str, Instant) -> ConnectionProbe,
) -> Result<ValidatedConnection, StaleReason> {
    if Instant::now() >= deadline {
        return Err(StaleReason::HealthFailed);
    }
    if connection.validation_error().is_some() {
        return Err(StaleReason::Invalid);
    }
    let Some(before) = observe_process(connection.pid) else {
        return Err(StaleReason::ProcessDead);
    };
    if !image_name_matches(&before.image, image_name) {
        return Err(StaleReason::ImageMismatch);
    }
    if !connection.has_boot_identity() {
        return Err(StaleReason::BootIdentityInvalid);
    }
    if Instant::now() >= deadline {
        return Err(StaleReason::HealthFailed);
    }
    let address = format!("127.0.0.1:{}", connection.port);
    match prove_connection(&address, &connection.api_key, deadline) {
        ConnectionProbe::Cancelled | ConnectionProbe::HealthFailed => {
            return Err(StaleReason::HealthFailed);
        }
        ConnectionProbe::KeyRejected => return Err(StaleReason::KeyRejected),
        ConnectionProbe::Accepted => {}
    }
    let Some(after) = observe_process(connection.pid) else {
        return Err(StaleReason::ProcessChanged);
    };
    if before != after {
        return Err(StaleReason::ProcessChanged);
    }
    Ok(ValidatedConnection {
        connection,
        process_identity: before,
    })
}

fn validate_cancellable_with(
    connection: GatewayDiscoveryFile,
    image_name: &str,
    cancellation: &CancellationToken,
    mut observe_process: impl FnMut(u32) -> Option<ProcessIdentity>,
    prove_connection: impl FnOnce(&str, &str, Duration, &CancellationToken) -> ConnectionProbe,
) -> Result<ValidatedConnection, ValidationError> {
    if cancellation.is_cancelled() {
        return Err(ValidationError::Cancelled);
    }
    if connection.validation_error().is_some() {
        return Err(StaleReason::Invalid.into());
    }
    let Some(before_observation) = cancellation.run_if_active(|| observe_process(connection.pid))
    else {
        return Err(ValidationError::Cancelled);
    };
    let Some(before) = before_observation else {
        return Err(StaleReason::ProcessDead.into());
    };
    if cancellation.is_cancelled() {
        return Err(ValidationError::Cancelled);
    }
    if !image_name_matches(&before.image, image_name) {
        return Err(StaleReason::ImageMismatch.into());
    }
    if !connection.has_boot_identity() {
        return Err(StaleReason::BootIdentityInvalid.into());
    }
    let address = format!("127.0.0.1:{}", connection.port);
    let proof = prove_connection(&address, &connection.api_key, LIVENESS_BUDGET, cancellation);
    if cancellation.is_cancelled() || proof == ConnectionProbe::Cancelled {
        return Err(ValidationError::Cancelled);
    }
    match proof {
        ConnectionProbe::HealthFailed => return Err(StaleReason::HealthFailed.into()),
        ConnectionProbe::KeyRejected => return Err(StaleReason::KeyRejected.into()),
        ConnectionProbe::Accepted => {}
        ConnectionProbe::Cancelled => return Err(ValidationError::Cancelled),
    }
    let Some(after_observation) = cancellation.run_if_active(|| observe_process(connection.pid))
    else {
        return Err(ValidationError::Cancelled);
    };
    let Some(after) = after_observation else {
        return Err(StaleReason::ProcessChanged.into());
    };
    if cancellation.is_cancelled() {
        return Err(ValidationError::Cancelled);
    }
    if before != after {
        return Err(StaleReason::ProcessChanged.into());
    }
    Ok(ValidatedConnection {
        connection,
        process_identity: before,
    })
}

fn image_name_matches(image: &Path, expected: &str) -> bool {
    let Some(name) = image.file_name() else {
        return false;
    };
    image_file_name_matches(name, expected)
}

#[cfg(windows)]
fn image_file_name_matches(name: &OsStr, expected: &str) -> bool {
    name.to_string_lossy().eq_ignore_ascii_case(expected)
}

#[cfg(not(windows))]
fn image_file_name_matches(name: &OsStr, expected: &str) -> bool {
    name == OsStr::new(expected)
}

#[cfg(test)]
#[path = "validated-tests.rs"]
mod tests;
