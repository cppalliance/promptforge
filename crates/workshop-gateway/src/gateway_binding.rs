//! Atomically replaceable Gateway endpoint and credential state.
//!
//! Every Gateway-dependent Workshop path loads one immutable snapshot
//! containing the HTTP client, base URL, bearer, and generation.
//! A local-sidecar replacement builds the complete next snapshot before one
//! atomic store, then notifies long-lived tasks to reconnect. Explicitly
//! configured endpoints never receive an updater from the desktop shell.

mod publication;
mod shutdown;

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use arc_swap::ArcSwap;
use tokio::sync::watch;

use crate::gateway::{GatewayClient, GatewayError};

/// One immutable generation of every Gateway client credential.
pub struct GatewaySnapshot {
    /// HTTP and Realtime client used by Workshop routes and the heartbeat.
    client: GatewayClient,
    /// Normalized Gateway base URL paired with both clients.
    base_url: String,
    /// Bearer paired with `client`, exposed for consumers that authenticate
    /// outside the HTTP client.
    api_key: String,
    /// Monotonic generation assigned before this snapshot is published.
    generation: u64,
    /// Proven local Gateway boot, absent for an explicitly configured endpoint.
    identity: Option<shared_gateway_discovery::ValidatedConnection>,
}

impl fmt::Debug for GatewaySnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewaySnapshot")
            .field("client", &self.client)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("generation", &self.generation)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl GatewaySnapshot {
    /// The HTTP and Realtime client in this generation.
    #[must_use]
    pub fn client(&self) -> &GatewayClient {
        &self.client
    }

    /// The Gateway base URL in this generation.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The Gateway bearer in this generation.
    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// This snapshot's monotonic generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// Shared atomic Gateway snapshot and replacement notification.
#[derive(Clone)]
pub struct GatewayBinding {
    current: Arc<ArcSwap<GatewaySnapshot>>,
    changed: watch::Sender<u64>,
    publication: Arc<Mutex<PublicationState>>,
}

#[derive(Debug)]
struct PublicationState {
    next_generation: u64,
    closed: bool,
}

/// A failure to publish a replacement Gateway generation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GatewayPublicationError {
    /// The replacement clients could not be built.
    #[error(transparent)]
    #[non_exhaustive]
    Build(#[from] GatewayError),
    /// The binding has permanently revoked replacement publication.
    #[error("gateway replacement publication is permanently closed")]
    PublicationClosed,
}

impl fmt::Debug for GatewayBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayBinding")
            .field("current", &self.snapshot())
            .finish()
    }
}

impl GatewayBinding {
    /// Builds generation zero from one endpoint and credential pair.
    ///
    /// # Errors
    /// Returns [`GatewayError::Build`] if the HTTP client cannot be built.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn new(base_url: &str, api_key: &str) -> Result<Self, GatewayError> {
        Self::new_with_identity(base_url, api_key, None)
    }

    /// Builds generation zero with an optional validated local identity.
    ///
    /// # Errors
    /// Returns [`GatewayError::Build`] if the HTTP client cannot be built.
    pub fn new_with_identity(
        base_url: &str,
        api_key: &str,
        identity: Option<shared_gateway_discovery::ValidatedConnection>,
    ) -> Result<Self, GatewayError> {
        let snapshot = Arc::new(build_snapshot(base_url, api_key, 0, identity)?);
        Ok(Self {
            current: Arc::new(ArcSwap::from(snapshot)),
            changed: watch::channel(0).0,
            publication: Arc::new(Mutex::new(PublicationState {
                next_generation: 1,
                closed: false,
            })),
        })
    }

    /// Builds a binding around a client carrying test-specific timeouts.
    #[must_use]
    pub fn from_client(client: GatewayClient) -> Self {
        let base_url = client.base_url.clone();
        let api_key = client.api_key.clone();
        let snapshot = Arc::new(GatewaySnapshot {
            client,
            base_url,
            api_key,
            generation: 0,
            identity: None,
        });
        Self {
            current: Arc::new(ArcSwap::from(snapshot)),
            changed: watch::channel(0).0,
            publication: Arc::new(Mutex::new(PublicationState {
                next_generation: 1,
                closed: false,
            })),
        }
    }

    /// Loads one endpoint and credential generation atomically.
    #[must_use]
    pub fn snapshot(&self) -> Arc<GatewaySnapshot> {
        self.current.load_full()
    }

    /// Subscribes to replacements after loading the current generation.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// The currently published generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.snapshot().generation()
    }

    /// Builds and atomically publishes a replacement, then wakes consumers.
    ///
    /// # Errors
    /// Returns [`GatewayPublicationError::Build`] if the replacement HTTP
    /// client cannot initialize, or
    /// [`GatewayPublicationError::PublicationClosed`] after teardown.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn replace(&self, base_url: &str, api_key: &str) -> Result<(), GatewayPublicationError> {
        self.replace_with_identity(base_url, api_key, None)
    }

    /// Builds and atomically publishes a complete replacement generation.
    fn replace_with_identity(
        &self,
        base_url: &str,
        api_key: &str,
        identity: Option<shared_gateway_discovery::ValidatedConnection>,
    ) -> Result<(), GatewayPublicationError> {
        let snapshot = build_snapshot(base_url, api_key, 0, identity)?;
        self.publish_snapshot(snapshot)
    }

    fn publish_snapshot(
        &self,
        mut snapshot: GatewaySnapshot,
    ) -> Result<(), GatewayPublicationError> {
        let mut publication = self
            .publication
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if publication.closed {
            return Err(GatewayPublicationError::PublicationClosed);
        }
        let generation = publication.next_generation;
        publication.next_generation = publication.next_generation.saturating_add(1);
        snapshot.generation = generation;
        self.current.store(Arc::new(snapshot));
        self.changed.send_replace(generation);
        Ok(())
    }

    fn close_publication(&self) {
        self.publication
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
    }

    fn publication_closed(&self) -> bool {
        self.publication
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed
    }

    /// Creates the restricted handle the desktop host uses for sidecar updates.
    #[must_use]
    pub fn updater(&self) -> GatewayUpdater {
        GatewayUpdater {
            binding: self.clone(),
        }
    }
}

/// Restricted local-Gateway authority for an embedding desktop host.
///
/// Replacements accept only validated capabilities, and shutdown reads the
/// same current immutable snapshot as every Workshop consumer. Raw connection
/// files cannot cross the publication boundary:
///
/// ```compile_fail
/// use shared_gateway_discovery::GatewayDiscoveryFile;
///
/// # fn publish(
/// #     updater: &workshop_gateway::GatewayUpdater,
/// #     raw: &GatewayDiscoveryFile,
/// # ) -> Result<(), workshop_gateway::GatewayPublicationError> {
/// updater.replace_sidecar(raw)
/// # }
/// ```
#[derive(Clone)]
pub struct GatewayUpdater {
    binding: GatewayBinding,
}

impl fmt::Debug for GatewayUpdater {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayUpdater")
            .finish_non_exhaustive()
    }
}

impl GatewayUpdater {
    /// Atomically replaces the local Gateway port and bearer, waking every
    /// long-lived Workshop consumer only after the complete snapshot is live.
    ///
    /// # Errors
    /// Returns [`GatewayPublicationError::Build`] if the replacement HTTP
    /// client cannot initialize, or
    /// [`GatewayPublicationError::PublicationClosed`] after teardown revokes
    /// replacement publication.
    pub fn replace_sidecar(
        &self,
        connection: &shared_gateway_discovery::ValidatedConnection,
    ) -> Result<(), GatewayPublicationError> {
        self.binding.replace_with_identity(
            &format!("http://127.0.0.1:{}", connection.port()),
            connection.api_key(),
            Some(connection.clone()),
        )
    }

    /// Replaces the configured Gateway in the crate's integration fixtures
    /// without manufacturing a production sidecar capability.
    ///
    /// # Errors
    /// Returns [`GatewayPublicationError::Build`] if the replacement HTTP
    /// client cannot initialize, or
    /// [`GatewayPublicationError::PublicationClosed`] after teardown.
    #[cfg(feature = "test-fixtures")]
    pub fn replace_fixture(
        &self,
        base_url: &str,
        api_key: &str,
    ) -> Result<(), GatewayPublicationError> {
        self.binding.replace(base_url, api_key)
    }

    /// Permanently revokes replacement publication for this binding and every
    /// updater clone.
    pub fn close_publication(&self) {
        self.binding.close_publication();
    }

    /// Whether replacement publication has been permanently revoked.
    #[must_use]
    pub fn publication_closed(&self) -> bool {
        self.binding.publication_closed()
    }
}

/// Builds all clients before publication so URL and bearer never tear.
fn build_snapshot(
    base_url: &str,
    api_key: &str,
    generation: u64,
    identity: Option<shared_gateway_discovery::ValidatedConnection>,
) -> Result<GatewaySnapshot, GatewayError> {
    let client = GatewayClient::new(base_url, api_key)?;
    let base_url = client.base_url().to_owned();
    Ok(GatewaySnapshot {
        client,
        base_url,
        api_key: api_key.to_owned(),
        generation,
        identity,
    })
}

#[cfg(test)]
mod tests;
