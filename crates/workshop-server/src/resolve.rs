//! Gateway endpoint resolution: a live connection file in the run
//! directory first, explicit `[gateway]` config second.
//!
//! The sidecar gateway writes `gateway.json` after a successful bind (see
//! `shared-sidecar`), so a workshop that finds a live file attaches to
//! that gateway - loopback, WSL, or LAN become one topology. A stale file
//! is condemned with its reason (the probe removes it) and explicit config
//! takes over; with no live file and no explicit config there is nothing
//! to connect to, which is the plain [`ResolveError`].

use std::path::Path;

use shared_sidecar::{Resolution, SidecarError, StaleReason};

use crate::config::GatewayConfig;
use crate::protocol::Activity;
use crate::push::Push;

/// The gateway endpoint state construction connects to, and how it was
/// found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGateway {
    base_url: String,
    api_key: String,
    source: GatewaySource,
    stale: Option<StaleReason>,
}

/// Which source won gateway endpoint resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GatewaySource {
    /// A live `gateway.json` connection file in the run directory.
    ConnectionFile,
    /// Explicit `[gateway]` settings from `workshop.toml`.
    Config,
}

impl ResolvedGateway {
    /// The endpoint explicit config names, with no discovery: the bypass
    /// for a host that already holds its gateway endpoint - the merged
    /// gateway hosting the workshop in-process, whose own just-written
    /// connection file is not serving yet when the workshop spawns - or a
    /// test fixture.
    #[must_use]
    pub fn from_config(config: &GatewayConfig) -> Self {
        Self {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            source: GatewaySource::Config,
            stale: None,
        }
    }

    /// The resolved base URL, for example `http://127.0.0.1:8081`.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The resolved bearer key.
    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Which source won the resolution.
    #[must_use]
    pub fn source(&self) -> GatewaySource {
        self.source
    }

    /// Why a connection file was condemned on the way to the config
    /// fallback, when one was.
    #[must_use]
    pub fn stale(&self) -> Option<StaleReason> {
        self.stale
    }

    /// The winning source rendered for the status bar and the log.
    #[must_use]
    pub(crate) fn source_label(&self) -> &'static str {
        match self.source {
            GatewaySource::ConnectionFile => "connection file",
            GatewaySource::Config => "workshop.toml",
        }
    }
}

/// Gateway endpoint resolution found nothing to connect to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
#[error("no gateway configured or running{detail}")]
pub struct ResolveError {
    /// The rendered suffix: the stale-file note and the remedy.
    detail: String,
    /// Why the connection file was condemned, when one was.
    stale: Option<StaleReason>,
}

impl ResolveError {
    /// The failure with the stale-file note rendered in, when a file was
    /// condemned on the way.
    fn new(stale: Option<StaleReason>) -> Self {
        let note = stale
            .map(|reason| {
                format!(
                    " (removed a stale connection file: {})",
                    stale_clause(reason)
                )
            })
            .unwrap_or_default();
        Self {
            detail: format!(
                "{note}; start promptforge-gateway or set [gateway] base_url and api_key in workshop.toml"
            ),
            stale,
        }
    }

    /// Why the connection file was condemned, when one was: a wrong key,
    /// a dead pid, and a foreign image are different problems for the
    /// operator.
    #[must_use]
    pub fn stale(&self) -> Option<StaleReason> {
        self.stale
    }
}

/// Resolves the gateway endpoint for a loaded config: the live connection
/// file in the default run directory first, explicit `[gateway]` config
/// second.
///
/// # Errors
/// Returns [`ResolveError`] when no live connection file exists and the
/// config carries no explicit gateway.
pub(crate) fn resolve(config: &GatewayConfig) -> Result<ResolvedGateway, ResolveError> {
    resolve_with(shared_sidecar::default_run_dir().as_deref(), config, probe)
}

/// The production probe: `shared_sidecar`'s stale-detecting resolve.
fn probe(run_dir: &Path) -> Result<Resolution, SidecarError> {
    shared_sidecar::resolve(run_dir)
}

/// `resolve` against an explicit run directory and probe, so tests point
/// at a tempdir and at a probe that accepts the test binary's own image.
fn resolve_with(
    run_dir: Option<&Path>,
    config: &GatewayConfig,
    probe: fn(&Path) -> Result<Resolution, SidecarError>,
) -> Result<ResolvedGateway, ResolveError> {
    let mut stale = None;
    if let Some(run_dir) = run_dir {
        match probe(run_dir) {
            Ok(Resolution::Attach(file)) => {
                return Ok(ResolvedGateway {
                    base_url: format!("http://127.0.0.1:{}", file.port),
                    api_key: file.api_key,
                    source: GatewaySource::ConnectionFile,
                    stale: None,
                });
            }
            Ok(Resolution::Stale(reason)) => {
                tracing::warn!(
                    reason = stale_clause(reason),
                    "removed a stale gateway connection file"
                );
                stale = Some(reason);
            }
            // A read or cleanup I/O failure degrades discovery to the
            // config fallback; it never fails startup on its own.
            Err(error) => {
                tracing::warn!("could not resolve the gateway connection file: {error}");
            }
            // Absent, and any future resolution: nothing to attach to.
            _ => {}
        }
    }
    if is_explicit(config) {
        return Ok(ResolvedGateway {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            source: GatewaySource::Config,
            stale,
        });
    }
    Err(ResolveError::new(stale))
}

/// Reports the resolution outcome where the house surfaces startup state:
/// a condemned file's reason and the winning source on the status bus,
/// the same facts in the log.
pub(crate) fn report(gateway: &ResolvedGateway, push: &Push) {
    if let Some(reason) = gateway.stale() {
        push.push_status_update(
            "Stale gateway connection file",
            format!("{}; attaching from workshop.toml", stale_clause(reason)),
            Activity::General,
        );
    }
    push.push_status_update(
        "Connecting to gateway",
        format!(
            "base URL {} ({})",
            gateway.base_url(),
            gateway.source_label()
        ),
        Activity::General,
    );
    tracing::info!(
        base_url = %gateway.base_url(),
        source = gateway.source_label(),
        "gateway endpoint resolved"
    );
}

/// Whether the config names a gateway itself: a non-empty `base_url` is
/// explicit, an empty one (unset, or an unset `${PROMPTFORGE_GATEWAY_URL}`
/// interpolation) is not. The explicit fallback exists for the gateways
/// discovery cannot see - a LAN gateway; a local gateway writes a
/// connection file, which discovery finds first.
fn is_explicit(config: &GatewayConfig) -> bool {
    !config.base_url.is_empty()
}

/// Renders a stale reason as a user-facing clause: a wrong key, a dead
/// pid, and a foreign image are different problems for the operator.
fn stale_clause(reason: StaleReason) -> &'static str {
    match reason {
        StaleReason::Invalid => "the file was not valid",
        StaleReason::ProcessDead => "the recorded gateway process is dead",
        StaleReason::ImageMismatch => "the recorded pid belongs to another program",
        StaleReason::HealthFailed => "the recorded gateway does not answer",
        StaleReason::KeyRejected => "the file's key was rejected",
        _ => "the file is stale",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Read, Write as _};
    use std::net::TcpListener;

    use shared_sidecar::ConnectionFile;

    use crate::catalog::CatalogBus;
    use crate::menu::MenuBus;
    use crate::status::StatusBus;

    /// The test process's own image name, so the probe's pid and image
    /// checks pass and the test reaches the liveness probes.
    fn own_image_name() -> String {
        std::env::current_exe()
            .expect("current exe")
            .file_name()
            .expect("the exe has a file name")
            .to_string_lossy()
            .into_owned()
    }

    /// A probe running the real liveness gauntlet against the test
    /// binary's own image.
    fn probe_own_image(run_dir: &Path) -> Result<Resolution, SidecarError> {
        shared_sidecar::resolve_for_test(run_dir, &own_image_name())
    }

    /// A connection file pointing at the test process itself.
    fn live_file(port: u16, api_key: &str) -> ConnectionFile {
        ConnectionFile {
            port,
            api_key: api_key.to_owned(),
            pid: std::process::id(),
            epoch: 1_757_000_000,
            version: "0.2.0".to_owned(),
            started_at: "2026-09-03T12:00:00Z".to_owned(),
        }
    }

    /// A pid guaranteed dead: a short-lived child, reaped and dropped so
    /// no handle keeps the process object alive.
    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new(std::env::current_exe().expect("current exe"))
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a short-lived child");
        let pid = child.id();
        child.wait().expect("the child exits");
        drop(child);
        pid
    }

    /// A fixture gateway: answers `GET /health` with 200 and the key
    /// probe with 200 only when the bearer matches `expected_key`.
    fn fixture_gateway(expected_key: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let port = listener.local_addr().expect("fixture address").port();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 1024];
                let Ok(read) = stream.read(&mut buffer) else {
                    continue;
                };
                let request = String::from_utf8_lossy(&buffer[..read]);
                let response = if request.starts_with("GET /health ")
                    || request.contains(&format!("Authorization: Bearer {expected_key}\r\n"))
                {
                    &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"[..]
                } else {
                    &b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"[..]
                };
                let _ = stream.write_all(response);
            }
        });
        port
    }

    /// An explicit gateway config: a LAN URL, never the built-in default.
    fn explicit_config() -> GatewayConfig {
        GatewayConfig {
            base_url: "http://gateway.lan:9999".to_owned(),
            api_key: "config-key".to_owned(),
        }
    }

    #[test]
    fn a_live_connection_file_wins_over_explicit_config() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let port = fixture_gateway("file-key");
        live_file(port, "file-key")
            .write_to(dir.path())
            .expect("write");

        let resolved = resolve_with(Some(dir.path()), &explicit_config(), probe_own_image)
            .expect("a live file resolves");
        assert_eq!(resolved.source(), GatewaySource::ConnectionFile);
        assert_eq!(resolved.base_url(), format!("http://127.0.0.1:{port}"));
        assert_eq!(resolved.api_key(), "file-key");
        assert_eq!(resolved.stale(), None);
    }

    #[test]
    fn a_stale_file_is_cleaned_and_explicit_config_wins() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let file = ConnectionFile {
            pid: dead_pid(),
            ..live_file(1, "k")
        };
        file.write_to(dir.path()).expect("write");

        let resolved = resolve_with(Some(dir.path()), &explicit_config(), probe_own_image)
            .expect("explicit config is the fallback");
        assert_eq!(resolved.source(), GatewaySource::Config);
        assert_eq!(resolved.base_url(), "http://gateway.lan:9999");
        assert_eq!(resolved.api_key(), "config-key");
        assert_eq!(resolved.stale(), Some(StaleReason::ProcessDead));
        assert!(
            !shared_sidecar::connection_file_path(dir.path()).exists(),
            "the stale file was removed"
        );
    }

    #[test]
    fn a_wrong_key_is_reported_distinctly_from_a_dead_pid() {
        // Wrong key: the pid, image, and health checks pass; the key
        // probe rejects.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let port = fixture_gateway("right");
        live_file(port, "wrong")
            .write_to(dir.path())
            .expect("write");
        let resolved = resolve_with(Some(dir.path()), &explicit_config(), probe_own_image)
            .expect("explicit config is the fallback");
        assert_eq!(resolved.stale(), Some(StaleReason::KeyRejected));

        // Dead pid: the process check fails before any probe runs.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let file = ConnectionFile {
            pid: dead_pid(),
            ..live_file(1, "k")
        };
        file.write_to(dir.path()).expect("write");
        let resolved = resolve_with(Some(dir.path()), &explicit_config(), probe_own_image)
            .expect("explicit config is the fallback");
        assert_eq!(resolved.stale(), Some(StaleReason::ProcessDead));
    }

    #[test]
    fn no_file_and_no_explicit_config_is_the_plain_error() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config = GatewayConfig {
            base_url: String::new(),
            api_key: String::new(),
        };
        let error = resolve_with(Some(dir.path()), &config, probe_own_image)
            .expect_err("an empty base_url is not explicit config");
        assert!(
            error
                .to_string()
                .contains("no gateway configured or running"),
            "the error says it plainly: {error}"
        );
        assert_eq!(error.stale(), None, "no file existed to condemn");
    }

    #[test]
    fn an_explicitly_configured_default_url_is_honored() {
        // The well-known default URL written by hand is explicit config:
        // it names a gateway discovery cannot see (an SSH-tunneled remote,
        // a gateway whose connection-file write failed), so it must
        // resolve, not read as an unset value.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config = GatewayConfig {
            base_url: "http://127.0.0.1:8081".to_owned(),
            api_key: "config-key".to_owned(),
        };
        let resolved = resolve_with(Some(dir.path()), &config, probe_own_image)
            .expect("an explicitly configured URL resolves");
        assert_eq!(resolved.source(), GatewaySource::Config);
        assert_eq!(resolved.base_url(), "http://127.0.0.1:8081");
        assert_eq!(resolved.api_key(), "config-key");
    }

    #[test]
    fn a_stale_file_with_no_explicit_config_carries_the_reason_into_the_error() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let port = fixture_gateway("right");
        live_file(port, "wrong")
            .write_to(dir.path())
            .expect("write");
        let config = GatewayConfig {
            base_url: String::new(),
            api_key: String::new(),
        };
        let error = resolve_with(Some(dir.path()), &config, probe_own_image)
            .expect_err("no explicit config remains");
        assert_eq!(error.stale(), Some(StaleReason::KeyRejected));
        let message = error.to_string();
        assert!(
            message.contains("no gateway configured or running"),
            "the error says it plainly: {message}"
        );
        assert!(
            message.contains("key was rejected"),
            "the condemned file's reason is named: {message}"
        );
    }

    #[test]
    fn no_file_and_explicit_config_uses_the_config() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let resolved = resolve_with(Some(dir.path()), &explicit_config(), probe_own_image)
            .expect("explicit config resolves");
        assert_eq!(resolved.source(), GatewaySource::Config);
        assert_eq!(resolved.stale(), None);
    }

    /// A probe whose connection-file read fails: a directory sits where
    /// `gateway.json` belongs, so the read errors instead of answering.
    fn probe_read_failure(run_dir: &Path) -> Result<Resolution, SidecarError> {
        std::fs::create_dir(run_dir.join("gateway.json")).expect("the unreadable file plants");
        shared_sidecar::resolve_for_test(run_dir, &own_image_name())
    }

    #[test]
    fn a_probe_io_failure_degrades_to_the_config_fallback() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let resolved = resolve_with(Some(dir.path()), &explicit_config(), probe_read_failure)
            .expect("a probe failure never fails startup on its own");
        assert_eq!(resolved.source(), GatewaySource::Config);
        assert_eq!(resolved.base_url(), "http://gateway.lan:9999");
        assert_eq!(resolved.stale(), None, "nothing was condemned");
    }

    #[test]
    fn a_probe_io_failure_with_no_explicit_config_is_the_plain_error() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config = GatewayConfig {
            base_url: String::new(),
            api_key: String::new(),
        };
        let error = resolve_with(Some(dir.path()), &config, probe_read_failure)
            .expect_err("no explicit config remains after a probe failure");
        assert!(
            error
                .to_string()
                .contains("no gateway configured or running"),
            "the error says it plainly: {error}"
        );
        assert_eq!(error.stale(), None, "a probe failure is not a condemnation");
    }

    #[test]
    fn no_run_directory_skips_discovery() {
        // The production probe stands in: with no run directory it is
        // never called, so resolution is the config fallback or the plain
        // error, and the real run directory is never consulted.
        let resolved = resolve_with(None, &explicit_config(), probe)
            .expect("explicit config resolves without a run directory");
        assert_eq!(resolved.source(), GatewaySource::Config);
        assert_eq!(resolved.stale(), None);

        let config = GatewayConfig {
            base_url: String::new(),
            api_key: String::new(),
        };
        let error = resolve_with(None, &config, probe)
            .expect_err("no run directory and no explicit config is the plain error");
        assert!(
            error
                .to_string()
                .contains("no gateway configured or running"),
            "the error says it plainly: {error}"
        );
    }

    #[test]
    fn the_report_names_the_winning_source_and_a_condemned_file() {
        let status = StatusBus::new();
        let catalog = CatalogBus::new();
        let menu = MenuBus::new(catalog.clone(), None);
        let push = Push::new(status.clone(), catalog, menu);
        let mut receiver = status.subscribe();

        let resolved = ResolvedGateway {
            base_url: "http://127.0.0.1:4000".to_owned(),
            api_key: "k".to_owned(),
            source: GatewaySource::Config,
            stale: Some(StaleReason::KeyRejected),
        };
        report(&resolved, &push);

        let stale_frame = receiver.try_recv().expect("the stale note is reported");
        assert_eq!(stale_frame.label, "Stale gateway connection file");
        assert!(
            stale_frame.description.contains("key was rejected"),
            "the stale reason is named: {}",
            stale_frame.description
        );
        let connecting = receiver.try_recv().expect("the winning source is reported");
        assert_eq!(connecting.label, "Connecting to gateway");
        assert!(
            connecting.description.contains("http://127.0.0.1:4000")
                && connecting.description.contains("workshop.toml"),
            "the endpoint and its source are named: {}",
            connecting.description
        );
    }
}
