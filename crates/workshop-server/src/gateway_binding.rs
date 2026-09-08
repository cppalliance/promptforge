//! Atomically replaceable Gateway endpoint and credential state.
//!
//! Every Gateway-dependent Workshop path loads one immutable snapshot
//! containing the HTTP client, model client, base URL, bearer, and generation.
//! A local-sidecar replacement builds the complete next snapshot before one
//! atomic store, then notifies long-lived tasks to reconnect. Explicitly
//! configured endpoints never receive an updater from the desktop shell.

mod publication;
mod shutdown;

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use arc_swap::ArcSwap;
use promptforge_model_client::client::{
    GatewayClient as ModelClient, GatewayEndpoint, SecretString,
};
use tokio::sync::watch;

use crate::gateway::{GatewayClient, GatewayError};

/// One immutable generation of every Gateway client credential.
pub(crate) struct GatewaySnapshot {
    /// HTTP and Realtime client used by Workshop routes and the heartbeat.
    client: GatewayClient,
    /// Normalized Gateway base URL paired with both clients.
    base_url: String,
    /// Bearer paired with `client`, retained for the progress subscriber.
    api_key: String,
    /// Agent completion client built from the same URL and bearer.
    model_client: Option<ModelClient>,
    /// Monotonic generation assigned before this snapshot is published.
    generation: u64,
    /// Proven local Gateway boot, absent for an explicitly configured endpoint.
    identity: Option<shared_sidecar::ValidatedConnection>,
}

impl fmt::Debug for GatewaySnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewaySnapshot")
            .field("client", &self.client)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("model_client", &"<redacted>")
            .field("generation", &self.generation)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl GatewaySnapshot {
    /// The HTTP and Realtime client in this generation.
    pub(crate) fn client(&self) -> &GatewayClient {
        &self.client
    }

    /// The agent model client from the same endpoint and credential pair.
    pub(crate) fn model_client(&self) -> Option<ModelClient> {
        self.model_client.clone()
    }

    /// The Gateway base URL in this generation.
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The Gateway bearer in this generation.
    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    /// This snapshot's monotonic generation.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

/// Shared atomic Gateway snapshot and replacement notification.
#[derive(Clone)]
pub(crate) struct GatewayBinding {
    current: Arc<ArcSwap<GatewaySnapshot>>,
    next_generation: Arc<AtomicU64>,
    changed: watch::Sender<u64>,
    replacement: Arc<Mutex<()>>,
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
    #[cfg(test)]
    pub(crate) fn new(base_url: &str, api_key: &str) -> Result<Self, GatewayError> {
        Self::new_with_identity(base_url, api_key, None)
    }

    /// Builds generation zero with an optional validated local identity.
    pub(crate) fn new_with_identity(
        base_url: &str,
        api_key: &str,
        identity: Option<shared_sidecar::ValidatedConnection>,
    ) -> Result<Self, GatewayError> {
        let snapshot = Arc::new(build_snapshot(base_url, api_key, 0, identity)?);
        Ok(Self {
            current: Arc::new(ArcSwap::from(snapshot)),
            next_generation: Arc::new(AtomicU64::new(1)),
            changed: watch::channel(0).0,
            replacement: Arc::new(Mutex::new(())),
        })
    }

    /// Builds a binding around a client carrying test-specific timeouts.
    pub(crate) fn from_client(client: GatewayClient) -> Self {
        let model_client = model_client(&client.base_url, &client.api_key);
        let base_url = client.base_url.clone();
        let api_key = client.api_key.clone();
        let snapshot = Arc::new(GatewaySnapshot {
            client,
            base_url,
            api_key,
            model_client,
            generation: 0,
            identity: None,
        });
        Self {
            current: Arc::new(ArcSwap::from(snapshot)),
            next_generation: Arc::new(AtomicU64::new(1)),
            changed: watch::channel(0).0,
            replacement: Arc::new(Mutex::new(())),
        }
    }

    /// Loads one endpoint and credential generation atomically.
    pub(crate) fn snapshot(&self) -> Arc<GatewaySnapshot> {
        self.current.load_full()
    }

    /// Subscribes to replacements after loading the current generation.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// The currently published generation.
    pub(crate) fn generation(&self) -> u64 {
        self.snapshot().generation()
    }

    /// Builds and atomically publishes a replacement, then wakes consumers.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn replace(&self, base_url: &str, api_key: &str) -> Result<(), GatewayError> {
        self.replace_with_identity(base_url, api_key, None)
    }

    /// Builds and atomically publishes a complete replacement generation.
    fn replace_with_identity(
        &self,
        base_url: &str,
        api_key: &str,
        identity: Option<shared_sidecar::ValidatedConnection>,
    ) -> Result<(), GatewayError> {
        let snapshot = build_snapshot(base_url, api_key, 0, identity)?;
        let _replacement = self
            .replacement
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.publish_snapshot(snapshot);
        Ok(())
    }

    fn publish_snapshot(&self, mut snapshot: GatewaySnapshot) {
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        snapshot.generation = generation;
        self.current.store(Arc::new(snapshot));
        self.changed.send_replace(generation);
    }

    /// Creates the restricted handle the desktop host uses for sidecar updates.
    pub(crate) fn updater(&self) -> GatewayUpdater {
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
/// use shared_sidecar::ConnectionFile;
///
/// # fn publish(
/// #     updater: &workshop_server::GatewayUpdater,
/// #     raw: &ConnectionFile,
/// # ) -> Result<(), workshop_server::GatewayError> {
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
    /// Returns [`GatewayError::Build`] if the replacement HTTP client cannot
    /// initialize.
    pub fn replace_sidecar(
        &self,
        connection: &shared_sidecar::ValidatedConnection,
    ) -> Result<(), GatewayError> {
        self.binding.replace_with_identity(
            &format!("http://127.0.0.1:{}", connection.port()),
            connection.api_key(),
            Some(connection.clone()),
        )
    }

    /// Replaces the configured Gateway in the crate's integration fixtures
    /// without manufacturing a production sidecar capability.
    #[cfg(feature = "test-fixtures")]
    pub(crate) fn replace_fixture(
        &self,
        base_url: &str,
        api_key: &str,
    ) -> Result<(), GatewayError> {
        self.binding.replace(base_url, api_key)
    }
}

/// Builds all clients before publication so URL and bearer never tear.
fn build_snapshot(
    base_url: &str,
    api_key: &str,
    generation: u64,
    identity: Option<shared_sidecar::ValidatedConnection>,
) -> Result<GatewaySnapshot, GatewayError> {
    let client = GatewayClient::new(base_url, api_key)?;
    let base_url = client.base_url().to_owned();
    let model_client = model_client(&base_url, api_key);
    Ok(GatewaySnapshot {
        client,
        base_url,
        api_key: api_key.to_owned(),
        model_client,
        generation,
        identity,
    })
}

/// Builds the agent model client carried in a Gateway snapshot.
pub(crate) fn model_client(base_url: &str, api_key: &str) -> Option<ModelClient> {
    let key = match SecretString::new(api_key) {
        Ok(key) => key,
        Err(error) => {
            tracing::warn!(%error, "agent sessions disabled: gateway API key unusable");
            return None;
        }
    };
    let root = format!("{}/v1", base_url.trim_end_matches('/'));
    let endpoint = match GatewayEndpoint::new(&root) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            tracing::warn!(%error, "agent sessions disabled: gateway URL unusable");
            return None;
        }
    };
    Some(ModelClient::new(endpoint, key))
}

#[cfg(test)]
mod tests;
