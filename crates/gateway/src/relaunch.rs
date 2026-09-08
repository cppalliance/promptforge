//! The relaunch handoff: a second `promptforge-gateway` launch while one is
//! running opens the running gateway's Settings page instead of booting a
//! duplicate server.
//!
//! Before any startup side effect, a process attempts the distinct
//! process-lifetime lease. Its holder re-resolves the connection file and
//! either hands off to a validated owner or boots while retaining the lease.
//! A lease loser reads without cleanup until the owner publishes a validated
//! record, then hands off. It never deletes stale state or initializes
//! canonical logging.

use std::time::{Duration, Instant};

use crate::runner::ServeOptions;

const OWNER_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// What a launch does about an existing connection file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Relaunch {
    /// A live gateway owns the file: hand off with its Settings URL.
    OpenSettings(String),
    /// Nothing live owns the file: boot normally.
    Boot,
}

/// The process ownership decision made before Gateway startup side effects.
#[derive(Debug)]
pub enum GatewayStartup {
    /// This process owns the lifetime lease and may boot.
    Boot(shared_sidecar::GatewayInstanceLease),
    /// Another validated Gateway owns the lease, so this process hands off.
    OpenSettings(String),
}

/// A failure to establish Gateway startup ownership.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GatewayStartupError {
    /// No run directory exists in which process ownership can be established.
    #[error("no user profile directory found for the Gateway process lease")]
    NoRunDirectory,
    /// The operating-system lease could not be opened or attempted.
    #[error("establish Gateway process ownership")]
    Lease(#[source] shared_sidecar::SidecarError),
    /// The lease holder could not resolve existing shared connection state.
    #[error("resolve existing Gateway connection before startup")]
    Resolve(#[source] shared_sidecar::SidecarError),
    /// The lease owner did not publish a validated record in time.
    #[error("the Gateway process owner published no validated connection within {timeout:?}")]
    OwnerTimeout {
        /// The bounded wait that elapsed.
        timeout: Duration,
    },
}

/// Maps a connection-file resolution to the relaunch decision. Only a
/// fully live file hands off; `Absent` and every stale reason boot.
pub(crate) fn decide(resolution: &shared_sidecar::Resolution) -> Relaunch {
    match resolution {
        shared_sidecar::Resolution::Attach(file) => {
            // The file carries the real port of the loopback bind; URLs
            // normalize to a literal 127.0.0.1, never localhost.
            Relaunch::OpenSettings(crate::handoff::auth_url(
                &format!("http://127.0.0.1:{}", file.port),
                &file.api_key,
            ))
        }
        _ => Relaunch::Boot,
    }
}

fn settings_url(connection: &shared_sidecar::ValidatedConnection) -> String {
    crate::handoff::auth_url(
        &format!("http://127.0.0.1:{}", connection.port()),
        connection.api_key(),
    )
}

/// Establishes process-lifetime Gateway ownership before logging, stale
/// cleanup, recovery, bind, or publication.
///
/// A lease holder re-resolves the connection record and may clean stale
/// state. A lease loser only reads and validates, waiting up to `timeout` for
/// the owner to publish. It never mutates shared state.
///
/// # Errors
/// Returns [`GatewayStartupError`] when no run directory is available, the
/// operating-system lease cannot be attempted, existing shared connection
/// state cannot be resolved, or the owner publishes no validated connection
/// within `timeout`.
pub fn settle_gateway_startup(
    options: &ServeOptions,
    timeout: Duration,
) -> Result<GatewayStartup, GatewayStartupError> {
    let run_dir = options
        .run_dir
        .clone()
        .or_else(shared_sidecar::default_run_dir)
        .ok_or(GatewayStartupError::NoRunDirectory)?;
    match shared_sidecar::GatewayInstanceLease::try_acquire(&run_dir)
        .map_err(GatewayStartupError::Lease)?
    {
        Some(lease) => {
            let resolution =
                shared_sidecar::resolve(&run_dir).map_err(GatewayStartupError::Resolve)?;
            match decide(&resolution) {
                Relaunch::OpenSettings(url) => Ok(GatewayStartup::OpenSettings(url)),
                Relaunch::Boot => Ok(GatewayStartup::Boot(lease)),
            }
        }
        None => wait_for_owner(&run_dir, timeout),
    }
}

fn wait_for_owner(
    run_dir: &std::path::Path,
    timeout: Duration,
) -> Result<GatewayStartup, GatewayStartupError> {
    wait_for_owner_with(timeout, |deadline| {
        let connection = shared_sidecar::ConnectionFile::read(run_dir).ok()??;
        let validated =
            shared_sidecar::ValidatedConnection::validate_before(connection, deadline).ok()?;
        Some(settings_url(&validated))
    })
}

fn wait_for_owner_with(
    timeout: Duration,
    mut find_owner: impl FnMut(Instant) -> Option<String>,
) -> Result<GatewayStartup, GatewayStartupError> {
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return Err(GatewayStartupError::OwnerTimeout { timeout });
        }
        if let Some(url) = find_owner(deadline) {
            return Ok(GatewayStartup::OpenSettings(url));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(GatewayStartupError::OwnerTimeout { timeout });
        }
        std::thread::sleep(OWNER_POLL_INTERVAL.min(remaining));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection file as a live gateway would write it.
    fn live_file() -> shared_sidecar::ConnectionFile {
        shared_sidecar::ConnectionFile {
            port: 8081,
            api_key: "abc123".to_owned(),
            pid: std::process::id(),
            epoch: 1_757_000_000,
            version: "0.2.0".to_owned(),
            started_at: "2026-09-03T12:00:00Z".to_owned(),
        }
    }

    #[test]
    fn a_live_file_hands_off_its_settings_url() {
        let decision = decide(&shared_sidecar::Resolution::Attach(live_file()));
        assert_eq!(
            decision,
            Relaunch::OpenSettings("http://127.0.0.1:8081/auth?key=abc123".to_owned()),
            "the handoff URL targets the live gateway's one-time /auth redirect"
        );
    }

    #[test]
    fn a_live_file_with_a_query_special_key_encodes_the_url() {
        let file = shared_sidecar::ConnectionFile {
            api_key: "a&b=c d".to_owned(),
            ..live_file()
        };
        let decision = decide(&shared_sidecar::Resolution::Attach(file));
        assert_eq!(
            decision,
            Relaunch::OpenSettings("http://127.0.0.1:8081/auth?key=a%26b%3Dc+d".to_owned()),
            "a configured key survives the handoff"
        );
    }

    #[test]
    fn absent_and_stale_files_boot_normally() {
        assert_eq!(decide(&shared_sidecar::Resolution::Absent), Relaunch::Boot);
        for reason in [
            shared_sidecar::StaleReason::Invalid,
            shared_sidecar::StaleReason::ProcessDead,
            shared_sidecar::StaleReason::ImageMismatch,
            shared_sidecar::StaleReason::HealthFailed,
            shared_sidecar::StaleReason::KeyRejected,
        ] {
            assert_eq!(
                decide(&shared_sidecar::Resolution::Stale(reason)),
                Relaunch::Boot,
                "stale ({reason:?}) boots rather than handing off"
            );
        }
    }

    #[test]
    fn an_empty_run_dir_settles_to_boot_with_the_lease_held() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let options = ServeOptions::new(None, None::<crate::ProfileName>)
            .with_run_dir(temp.path().to_path_buf());
        assert!(
            matches!(
                settle_gateway_startup(&options, Duration::from_millis(100))
                    .expect("startup ownership settles"),
                GatewayStartup::Boot(_)
            ),
            "an absent connection record lets the lease holder boot"
        );
    }

    #[test]
    fn a_stale_file_boots_and_is_cleaned() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        // A dead pid: a short-lived child, reaped and dropped.
        let mut child = std::process::Command::new(std::env::current_exe().expect("current exe"))
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a short-lived child");
        let pid = child.id();
        child.wait().expect("the child exits");
        drop(child);
        let file = shared_sidecar::ConnectionFile { pid, ..live_file() };
        file.write_to(temp.path()).expect("write fixture");
        let options = ServeOptions::new(None, None::<crate::ProfileName>)
            .with_run_dir(temp.path().to_path_buf());

        assert!(
            matches!(
                settle_gateway_startup(&options, Duration::from_millis(100))
                    .expect("startup ownership settles"),
                GatewayStartup::Boot(_)
            ),
            "a stale file lets the lease holder boot"
        );
        assert!(
            !shared_sidecar::connection_file_path(temp.path()).exists(),
            "the stale file was deleted so the boot rewrites it cleanly"
        );
    }

    #[test]
    fn a_connection_resolution_failure_stops_startup_with_its_source() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let connection_path = shared_sidecar::connection_file_path(temp.path());
        std::fs::create_dir_all(&connection_path).expect("create unreadable connection fixture");
        let options = ServeOptions::new(None, None::<crate::ProfileName>)
            .with_run_dir(temp.path().to_path_buf());

        let error = settle_gateway_startup(&options, Duration::from_millis(100))
            .expect_err("an unreadable connection record blocks startup");

        assert!(
            matches!(
                error,
                GatewayStartupError::Resolve(shared_sidecar::SidecarError::Read { .. })
            ),
            "the startup error preserves the connection read failure: {error:?}"
        );
        assert!(
            connection_path.is_dir(),
            "failed resolution does not mutate uncertain shared state"
        );
    }

    #[test]
    fn a_slow_owner_validation_obeys_the_single_wait_deadline() {
        let timeout = Duration::from_millis(100);
        let started = Instant::now();

        let error = wait_for_owner_with(timeout, |deadline| {
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            None
        })
        .expect_err("the owner never validates");

        assert!(matches!(error, GatewayStartupError::OwnerTimeout { .. }));
        assert!(
            started.elapsed() < Duration::from_millis(300),
            "the ownership wait does not add another poll or validation budget"
        );
    }
}
