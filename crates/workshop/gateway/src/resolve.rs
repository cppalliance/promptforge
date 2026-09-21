//! Gateway endpoint resolution: a live gateway discovery file in the run
//! directory first, explicit `[gateway]` config second.
//!
//! The sidecar gateway writes `gateway.json` after a successful bind (see
//! `gateway-api-discovery`), so a workshop that finds a live file attaches to
//! that gateway - loopback, WSL, or LAN become one topology. A stale file
//! is condemned with its reason (the probe removes it) and explicit config
//! takes over; with no live file and no explicit config there is nothing
//! to connect to, which is the plain [`ResolveError`].

use std::path::Path;

use gateway_api_discovery::{Resolution, SidecarError, StaleReason, ValidatedConnection};

use workshop_protocol::Activity;
use workshop_registry::Push;
use workshop_support::GatewayConfig;

/// The gateway endpoint state construction connects to, and how it was
/// found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGateway {
    base_url: String,
    api_key: String,
    identity: Option<ValidatedConnection>,
    source: GatewaySource,
    stale: Option<StaleReason>,
}

/// Which source won gateway endpoint resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GatewaySource {
    /// A live `gateway.json` gateway discovery file in the run directory.
    GatewayDiscoveryFile,
    /// Explicit `[gateway]` settings from `workshop.toml`.
    Config,
}

impl ResolvedGateway {
    /// The endpoint explicit config names, with no discovery: the bypass
    /// for a host that already holds its gateway endpoint, or a test
    /// fixture.
    #[must_use]
    pub fn from_config(config: &GatewayConfig) -> Self {
        Self {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            identity: None,
            source: GatewaySource::Config,
            stale: None,
        }
    }

    /// The endpoint a validated local Gateway boot published, for a host
    /// holding its own validated identity (a test fixture).
    #[cfg(feature = "test-fixtures")]
    #[must_use]
    pub fn from_validated(identity: ValidatedConnection) -> Self {
        Self {
            base_url: format!("http://127.0.0.1:{}", identity.port()),
            api_key: identity.api_key().to_owned(),
            identity: Some(identity),
            source: GatewaySource::GatewayDiscoveryFile,
            stale: None,
        }
    }

    /// The same resolution with its API served from `base_url` instead of
    /// the identity's own port: a test host whose shutdown authority (the
    /// validated fixture process) and API surface (a mock router) are two
    /// processes.
    #[cfg(feature = "test-fixtures")]
    #[must_use]
    pub fn with_base_url(self, base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            ..self
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

    /// The validated local Gateway boot, when discovery won.
    #[must_use]
    pub fn identity(&self) -> Option<&ValidatedConnection> {
        self.identity.as_ref()
    }

    /// Which source won the resolution.
    #[must_use]
    pub fn source(&self) -> GatewaySource {
        self.source
    }

    /// Why a gateway discovery file was condemned on the way to the config
    /// fallback, when one was.
    #[must_use]
    pub fn stale(&self) -> Option<StaleReason> {
        self.stale
    }

    /// The winning source rendered for the status bar and the log.
    #[must_use]
    pub(crate) fn source_label(&self) -> &'static str {
        match self.source {
            GatewaySource::GatewayDiscoveryFile => "gateway discovery file",
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
    /// Why the gateway discovery file was condemned, when one was.
    stale: Option<StaleReason>,
}

impl ResolveError {
    /// The failure with the stale-file note rendered in, when a file was
    /// condemned on the way.
    fn new(stale: Option<StaleReason>) -> Self {
        let note = stale
            .map(|reason| {
                format!(
                    " (removed a stale gateway discovery file: {})",
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

    /// Why the gateway discovery file was condemned, when one was: a wrong key,
    /// a dead pid, and a foreign image are different problems for the
    /// operator.
    #[must_use]
    pub fn stale(&self) -> Option<StaleReason> {
        self.stale
    }
}

/// Resolves the gateway endpoint for a loaded config: the live gateway
/// discovery file in the default run directory first, explicit `[gateway]`
/// config second.
///
/// # Errors
/// Returns [`ResolveError`] when no live gateway discovery file exists and the
/// config names no explicit gateway.
pub fn resolve(config: &GatewayConfig) -> Result<ResolvedGateway, ResolveError> {
    resolve_with(
        gateway_api_discovery::default_run_dir().as_deref(),
        config,
        probe,
    )
}

/// The production probe: `gateway_api_discovery`'s stale-detecting resolve.
fn probe(run_dir: &Path) -> Result<Resolution, SidecarError> {
    gateway_api_discovery::resolve(run_dir)
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
            Ok(Resolution::Attach(file)) => match validate_resolved(file) {
                Ok(identity) => {
                    return Ok(ResolvedGateway {
                        base_url: format!("http://127.0.0.1:{}", identity.port()),
                        api_key: identity.api_key().to_owned(),
                        identity: Some(identity),
                        source: GatewaySource::GatewayDiscoveryFile,
                        stale: None,
                    });
                }
                Err(reason) => stale = Some(reason),
            },
            Ok(Resolution::Stale(reason)) => {
                tracing::warn!(
                    reason = stale_clause(reason),
                    "removed a stale gateway discovery file"
                );
                stale = Some(reason);
            }
            // A read or cleanup I/O failure degrades discovery to the
            // config fallback; it never fails startup on its own.
            Err(error) => {
                tracing::warn!("could not resolve the gateway discovery file: {error}");
            }
            // Absent, and any future resolution: nothing to attach to.
            _ => {}
        }
    }
    if is_explicit(config) {
        return Ok(ResolvedGateway {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            identity: None,
            source: GatewaySource::Config,
            stale,
        });
    }
    Err(ResolveError::new(stale))
}

/// Reifies the shared resolver's live result as the capability stored in
/// Workshop's immutable Gateway snapshot.
fn validate_resolved(
    file: gateway_api_discovery::GatewayDiscoveryFile,
) -> Result<ValidatedConnection, StaleReason> {
    ValidatedConnection::validate(file)
}

/// Reports the resolution outcome where the house surfaces startup state:
/// a condemned file's reason and the winning source on the status bus,
/// the same facts in the log.
pub fn report(gateway: &ResolvedGateway, push: &Push) {
    if let Some(reason) = gateway.stale() {
        push.push_status_update(
            "Stale gateway discovery file",
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
/// gateway discovery file, which discovery finds first.
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
#[path = "resolve-tests.rs"]
mod tests;
