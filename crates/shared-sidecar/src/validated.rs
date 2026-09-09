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

/// Budget for proving health without condemning one transient failure.
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
/// one TCP connection carries both network checks. This closes the
/// health-to-bearer replacement gap and rejects pid reuse during that
/// interval. The capability is a point-in-time proof and makes no claim
/// that the process remains live after validation returns.
///
/// The bearer is deliberately absent from [`Debug`](fmt::Debug) output.
///
/// ```compile_fail
/// use shared_sidecar::ValidatedConnection;
///
/// let _raw = ValidatedConnection {
///     connection: panic!("external code cannot fill the private field"),
/// };
/// ```
///
/// No test-fixture feature exposes another production-capability
/// constructor:
///
/// ```compile_fail
/// use shared_sidecar::{GatewayDiscoveryFile, ValidatedConnection};
///
/// let raw = GatewayDiscoveryFile {
///     port: 8081,
///     api_key: "forged".into(),
///     pid: std::process::id(),
///     epoch: 1,
///     version: "test".into(),
///     started_at: "2026-09-07T00:00:00Z".into(),
/// };
/// let _ = ValidatedConnection::validate_for_test(raw);
/// ```
///
/// The crate-private named validator is equally unavailable:
///
/// ```compile_fail
/// use shared_sidecar::{GatewayDiscoveryFile, ValidatedConnection};
///
/// let raw = GatewayDiscoveryFile {
///     port: 8081,
///     api_key: "forged".into(),
///     pid: std::process::id(),
///     epoch: 1,
///     version: "test".into(),
///     started_at: "2026-09-07T00:00:00Z".into(),
/// };
/// let _ = ValidatedConnection::validate_named(raw, "my-test-binary");
/// ```
#[derive(Clone, PartialEq, Eq)]
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
mod tests {
    use super::*;

    use std::io::{Read, Write as _};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};

    use crate::GatewayDiscoveryFile;

    fn own_image_name() -> String {
        std::env::current_exe()
            .expect("current exe")
            .file_name()
            .expect("the exe has a file name")
            .to_string_lossy()
            .into_owned()
    }

    fn connection(port: u16, api_key: &str) -> GatewayDiscoveryFile {
        GatewayDiscoveryFile {
            port,
            api_key: api_key.to_owned(),
            pid: std::process::id(),
            epoch: 1_778_000_000,
            version: "0.2.0".to_owned(),
            started_at: "2026-05-05T12:00:00Z".to_owned(),
        }
    }

    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new(std::env::current_exe().expect("current exe"))
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a short-lived child");
        let pid = child.id();
        child.wait().expect("the child exits");
        pid
    }

    fn fixture_gateway(expected_key: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let port = listener.local_addr().expect("fixture address").port();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                for _ in 0..2 {
                    let mut buffer = [0_u8; 1024];
                    let Ok(read) = stream.read(&mut buffer) else {
                        break;
                    };
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let accepted = request.starts_with("GET /health ")
                        || request.contains(&format!("Authorization: Bearer {expected_key}\r\n"));
                    let response = if accepted {
                        &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"[..]
                    } else {
                        &b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"[..]
                    };
                    if stream.write_all(response).is_err() {
                        break;
                    }
                }
            }
        });
        port
    }

    #[test]
    fn a_wrong_process_image_cannot_create_a_capability() {
        let file = connection(1, "key");

        assert_eq!(
            ValidatedConnection::validate_named(file, "not-the-test-binary"),
            Err(StaleReason::ImageMismatch)
        );
    }

    #[test]
    fn a_dead_process_cannot_create_a_capability() {
        let file = GatewayDiscoveryFile {
            pid: dead_pid(),
            ..connection(1, "key")
        };

        assert_eq!(
            ValidatedConnection::validate_named(file, &own_image_name()),
            Err(StaleReason::ProcessDead)
        );
    }

    #[test]
    fn a_stale_boot_identity_cannot_create_a_capability() {
        let missing_epoch = GatewayDiscoveryFile {
            epoch: 0,
            ..connection(1, "key")
        };
        assert_eq!(
            ValidatedConnection::validate_named(missing_epoch, &own_image_name()),
            Err(StaleReason::BootIdentityInvalid)
        );

        let missing_start = GatewayDiscoveryFile {
            started_at: String::new(),
            ..connection(1, "key")
        };
        assert_eq!(
            ValidatedConnection::validate_named(missing_start, &own_image_name()),
            Err(StaleReason::BootIdentityInvalid)
        );
    }

    #[test]
    fn an_invalid_raw_connection_cannot_create_a_capability() {
        let file = GatewayDiscoveryFile {
            port: 0,
            ..connection(1, "key")
        };

        assert_eq!(
            ValidatedConnection::validate_named(file, &own_image_name()),
            Err(StaleReason::Invalid)
        );
    }

    #[test]
    fn failed_health_cannot_create_a_capability() {
        let file = connection(1, "key");

        assert_eq!(
            ValidatedConnection::validate_named(file, &own_image_name()),
            Err(StaleReason::HealthFailed)
        );
    }

    #[test]
    fn an_absolute_validation_deadline_bounds_a_slow_owner() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind slow owner");
        let port = listener.local_addr().expect("slow owner address").port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept validation");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            std::thread::sleep(Duration::from_secs(1));
        });
        let timeout = Duration::from_millis(100);
        let started = Instant::now();

        let result = ValidatedConnection::validate_named_before(
            connection(port, "key"),
            &own_image_name(),
            started + timeout,
        );

        assert_eq!(result, Err(StaleReason::HealthFailed));
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the caller's deadline caps the real network proof"
        );
    }

    #[test]
    fn a_rejected_bearer_cannot_create_a_capability() {
        let port = fixture_gateway("accepted");
        let file = connection(port, "rejected");

        assert_eq!(
            ValidatedConnection::validate_named(file, &own_image_name()),
            Err(StaleReason::KeyRejected)
        );
    }

    #[test]
    fn a_new_boot_with_the_same_port_and_key_creates_a_distinct_capability() {
        let port = fixture_gateway("stable-key");
        let original =
            ValidatedConnection::validate_named(connection(port, "stable-key"), &own_image_name())
                .expect("the original connection validates");
        let replacement_file = GatewayDiscoveryFile {
            epoch: original.epoch() + 1,
            started_at: "2026-05-05T12:00:01Z".to_owned(),
            ..connection(port, "stable-key")
        };
        let replacement = ValidatedConnection::validate_named(replacement_file, &own_image_name())
            .expect("the replacement connection validates");

        assert_eq!(replacement.port(), original.port());
        assert_eq!(replacement.api_key(), original.api_key());
        assert!(!replacement.same_boot(&original));
    }

    #[test]
    fn debug_output_never_contains_the_bearer() {
        let secret = "capability-secret";
        let port = fixture_gateway(secret);
        let mut raw = connection(port, secret);
        raw.version = format!("version-{secret}\r\n");
        raw.started_at = format!("started-{secret}\t");
        let validated = ValidatedConnection::validate_named(raw, &own_image_name())
            .expect("the connection validates");

        let debug = format!("{validated:?}");
        assert!(!debug.contains(secret), "debug output redacts the bearer");
        assert!(
            !debug.contains(['\r', '\n', '\t']),
            "untrusted metadata cannot inject debug output"
        );
        assert!(debug.contains(&port.to_string()), "the endpoint is visible");
    }

    #[test]
    fn a_process_boot_change_during_the_network_proof_is_rejected() {
        let image = std::path::PathBuf::from(own_image_name());
        let before = ProcessIdentity::for_test(image.clone(), 41);
        let after = ProcessIdentity::for_test(image, 42);
        let mut observations = [Some(before), Some(after)].into_iter();

        assert_eq!(
            validate_with(
                connection(8081, "key"),
                &own_image_name(),
                |_| observations.next().flatten(),
                |_, _, _| ConnectionProbe::Accepted,
            ),
            Err(StaleReason::ProcessChanged),
            "a reused pid cannot complete a mixed proof"
        );
    }

    #[test]
    fn cancellation_inside_the_first_identity_probe_prevents_observation() {
        let cancellation = crate::CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let image_name = own_image_name();
        let identity = ProcessIdentity::for_test(std::path::PathBuf::from(&image_name), 41);
        let observations = Arc::new(AtomicUsize::new(0));
        let worker_observations = Arc::clone(&observations);
        let (entered, blocked) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            validate_cancellable_with(
                connection(8081, "key"),
                &image_name,
                &worker_cancellation,
                |_| {
                    entered.send(()).expect("announce identity probe");
                    if !worker_cancellation.wait_timeout(Duration::from_secs(30)) {
                        worker_observations.fetch_add(1, Ordering::SeqCst);
                    }
                    Some(identity.clone())
                },
                |_, _, _, _| ConnectionProbe::Accepted,
            )
        });
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("the first process identity probe blocks deterministically");

        let started = std::time::Instant::now();
        cancellation.cancel();
        let result = worker.join().expect("the validation worker joins");

        assert!(
            started.elapsed() < Duration::from_millis(250),
            "cancellation wakes the in-progress process identity probe"
        );
        assert!(matches!(result, Err(ValidationError::Cancelled)));
        assert_eq!(
            observations.load(Ordering::SeqCst),
            0,
            "no process observation occurs after cancellation"
        );
    }

    #[test]
    fn cancellation_during_validation_prevents_the_second_identity_observation() {
        let cancellation = crate::CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let image_name = own_image_name();
        let identity = ProcessIdentity::for_test(std::path::PathBuf::from(&image_name), 41);
        let observations = Arc::new(AtomicUsize::new(0));
        let worker_observations = Arc::clone(&observations);
        let (entered, blocked) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            validate_cancellable_with(
                connection(8081, "key"),
                &image_name,
                &worker_cancellation,
                |_| {
                    worker_observations.fetch_add(1, Ordering::SeqCst);
                    Some(identity.clone())
                },
                |_, _, _, cancellation| {
                    entered.send(()).expect("announce blocked validation");
                    let _ = cancellation.wait_timeout(Duration::from_secs(30));
                    ConnectionProbe::Accepted
                },
            )
        });
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("the validation phase blocks deterministically");

        let started = std::time::Instant::now();
        cancellation.cancel();
        let result = worker.join().expect("the validation worker joins");

        assert!(
            started.elapsed() < Duration::from_millis(250),
            "cancellation bounds the blocked validation"
        );
        assert!(matches!(result, Err(ValidationError::Cancelled)));
        assert_eq!(
            observations.load(Ordering::SeqCst),
            1,
            "validation performs no post-cancel process probe"
        );
    }

    #[test]
    fn forged_file_identity_cannot_alias_a_reused_process() {
        let raw = connection(8081, "key");
        let image = std::path::PathBuf::from(own_image_name());
        let first_identity = ProcessIdentity::for_test(image.clone(), 41);
        let second_identity = ProcessIdentity::for_test(image, 42);
        let first = validate_with(
            raw.clone(),
            &own_image_name(),
            |_| Some(first_identity.clone()),
            |_, _, _| ConnectionProbe::Accepted,
        )
        .expect("the first coherent proof validates");
        let second = validate_with(
            raw,
            &own_image_name(),
            |_| Some(second_identity.clone()),
            |_, _, _| ConnectionProbe::Accepted,
        )
        .expect("the replacement's coherent proof validates");

        assert!(
            !first.same_boot(&second),
            "identical attacker-controlled file fields cannot forge process identity"
        );
    }

    #[test]
    fn validation_errors_never_contain_the_bearer_or_metadata() {
        let secret = "capability-secret";
        let mut raw = connection(8081, secret);
        raw.version = format!("version-{secret}\r\n");
        raw.started_at = format!("started-{secret}\t");
        let image = std::path::PathBuf::from(own_image_name());
        let identity = ProcessIdentity::for_test(image, 41);
        let error = validate_with(
            raw,
            &own_image_name(),
            |_| Some(identity.clone()),
            |_, _, _| ConnectionProbe::KeyRejected,
        )
        .expect_err("the bearer is rejected");

        let debug = format!("{error:?}");
        let display = format!("{error}");
        for rendered in [debug, display] {
            assert!(!rendered.contains(secret), "errors redact the bearer");
            assert!(
                !rendered.contains(['\r', '\n', '\t']),
                "untrusted metadata cannot inject errors"
            );
        }
    }
}
