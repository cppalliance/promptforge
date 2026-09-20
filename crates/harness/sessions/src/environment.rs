//! The session run's environment: the bindings a client pushes across the
//! door (the gateway, the chat catalog, the host snapshot), the resources
//! the harness builds from one gateway generation (the capability
//! registry of first-party capabilities and the model client), and the
//! launch-time resolution of the client's selected model into the run's
//! context.
//!
//! Everything here arrives as data. The harness never resolves a gateway,
//! reads a menu, or names a workspace crate: the client pushes a
//! [`GatewayBinding`] at startup and on every replacement, a
//! [`CatalogBinding`] whenever its chat-capable model list changes, and a
//! [`HostSnapshot`] whenever its selection or roots change. Sessions
//! observe generation changes through watches and read the latest value
//! at launch.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use harness_capabilities::CapabilityRegistry;
use harness_models::{
    CompletionError, GatewayClient, GatewayEndpoint, SecretString, fetch_model_catalog,
};
use harness_web::Web;
use promptforge_api_types::models::{ModelDescriptor, ModelId};
use promptforge_api_types::tools::ToolError;
use tokio::sync::watch;

/// One generation of the gateway a client has bound the harness to.
///
/// The client pushes a binding at startup and on every gateway
/// replacement; the harness rebuilds its capability registry and model
/// client when `generation` changes. The binding is data pushed across
/// the door: the harness never resolves a gateway itself.
#[derive(Clone, PartialEq, Eq)]
pub struct GatewayBinding {
    /// The gateway's base URL.
    pub base_url: String,
    /// The bearer key paired with `base_url`.
    pub key: String,
    /// Monotonic generation the client assigns to each replacement.
    pub generation: u64,
}

impl GatewayBinding {
    /// The gateway's OpenAI-shaped API root: `base_url` with `/v1`.
    #[must_use]
    pub fn api_root(&self) -> String {
        format!("{}/v1", self.base_url.trim_end_matches('/'))
    }
}

impl fmt::Debug for GatewayBinding {
    /// The bearer key is never written to logs or `Debug` output.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayBinding")
            .field("base_url", &self.base_url)
            .field("key", &"<redacted>")
            .field("generation", &self.generation)
            .finish()
    }
}

/// One generation of the client's chat-capable model catalog.
///
/// A session freezes the catalog generation it launched under; a later
/// generation whose `models` differ retires the run and relaunches it
/// over the retained transcript once the accepted turn settles. An empty
/// `models` list means no chat-capable model exists at this generation:
/// a session waits on it rather than launching or relaunching a run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CatalogBinding {
    /// Monotonic generation the client assigns to each change.
    pub generation: u64,
    /// The chat-capable entries, as the gateway lists them; empty when
    /// none is available.
    pub models: Vec<serde_json::Value>,
}

/// The host state a run reads at launch: what the `ui()` global serves
/// and the model the prompt's roles bind to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostSnapshot {
    /// The client's selected model id, when one is selected.
    pub selected_model: Option<String>,
    /// The workspace roots the client has granted; the first is the
    /// `ui()` snapshot's `workspace_root`.
    pub workspace_roots: Vec<PathBuf>,
}

impl HostSnapshot {
    /// The `ui()` snapshot: `selected_model` and `workspace_root`, each
    /// `null` when absent.
    #[must_use]
    pub fn ui(&self) -> serde_json::Value {
        let root = self
            .workspace_roots
            .first()
            .map(|root| root.display().to_string());
        serde_json::json!({ "selected_model": self.selected_model, "workspace_root": root })
    }
}

/// Builds a registry holding the first-party capabilities for one gateway
/// generation: today `promptforge/web`, built from the gateway's API root
/// (`root`, the OpenAI-shaped `/v1` base) and bearer `token`. The
/// registry is rebuilt when the gateway generation changes, so a
/// replacement gateway's root and key reach the contributed tools.
///
/// # Errors
/// Returns the web capability's own [`ToolError`] when `root` is not a
/// valid gateway API root or `token` is empty.
pub fn first_party_registry(root: &str, token: &str) -> Result<CapabilityRegistry, ToolError> {
    let web = Web::new(root, token)?;
    let mut registry = CapabilityRegistry::new();
    // A single registration cannot collide; the registry's error is
    // unreachable on this path, and dropping it keeps the signature to the
    // one failure a caller can act on.
    let _ = registry.register(Arc::new(web));
    Ok(registry)
}

/// Builds the model client for one gateway binding, or `None` - reported
/// as an unusable gateway at launch - when the key or URL cannot build
/// one.
#[must_use]
pub fn gateway_client(binding: &GatewayBinding) -> Option<GatewayClient> {
    let key = match SecretString::new(binding.key.as_str()) {
        Ok(key) => key,
        Err(error) => {
            tracing::warn!(%error, "agent sessions disabled: gateway API key unusable");
            return None;
        }
    };
    let endpoint = match GatewayEndpoint::new(&binding.api_root()) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            tracing::warn!(%error, "agent sessions disabled: gateway URL unusable");
            return None;
        }
    };
    Some(GatewayClient::new(endpoint, key))
}

/// What the harness builds from one gateway generation and shares across
/// every run launched under it: the registry of first-party capabilities
/// and the model client. Rebuilt whole when the generation changes.
#[derive(Clone)]
pub struct GatewayResources {
    binding: GatewayBinding,
    registry: Option<Arc<CapabilityRegistry>>,
    client: Option<GatewayClient>,
}

impl fmt::Debug for GatewayResources {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayResources")
            .field("binding", &self.binding)
            .field("registry", &self.registry.is_some())
            .field("client", &self.client.is_some())
            .finish()
    }
}

impl GatewayResources {
    /// Builds the resources for `binding`. A binding whose root or key
    /// cannot build a capability or a client leaves that resource `None`;
    /// a launch under it is refused with the reason.
    #[must_use]
    pub fn build(binding: GatewayBinding) -> Self {
        let registry = match first_party_registry(&binding.api_root(), &binding.key) {
            Ok(registry) => Some(Arc::new(registry)),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "agent sessions degraded: the gateway cannot build promptforge/web"
                );
                None
            }
        };
        let client = gateway_client(&binding);
        Self {
            binding,
            registry,
            client,
        }
    }

    /// The binding these resources were built from.
    #[must_use]
    pub fn binding(&self) -> &GatewayBinding {
        &self.binding
    }

    /// The generation these resources were built for.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.binding.generation
    }

    /// The registry of first-party capabilities, when the binding could
    /// build it.
    #[must_use]
    pub fn registry(&self) -> Option<&Arc<CapabilityRegistry>> {
        self.registry.as_ref()
    }

    /// The model client, when the binding could build it.
    #[must_use]
    pub fn client(&self) -> Option<&GatewayClient> {
        self.client.as_ref()
    }
}

/// The bindings one harness holds for every session it serves, each
/// replaceable by the client and each watched by the sessions.
///
/// A generation watch carries the latest generation (`None` before the
/// first push); a session that observes a change reads the value behind
/// it. The gateway's resources are rebuilt only when its generation
/// changes: pushing the same generation twice is a no-op.
pub struct Bindings {
    gateway: RwLock<Option<Arc<GatewayResources>>>,
    gateway_generation: watch::Sender<Option<u64>>,
    catalog: RwLock<Option<CatalogBinding>>,
    catalog_generation: watch::Sender<Option<u64>>,
    host: RwLock<HostSnapshot>,
}

impl fmt::Debug for Bindings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Bindings")
            .field("gateway", &self.gateway())
            .field("catalog_generation", &self.catalog().map(|c| c.generation))
            .field("host", &self.host())
            .finish()
    }
}

impl Default for Bindings {
    fn default() -> Self {
        Self::new()
    }
}

impl Bindings {
    /// Bindings with nothing pushed yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            gateway: RwLock::new(None),
            gateway_generation: watch::Sender::new(None),
            catalog: RwLock::new(None),
            catalog_generation: watch::Sender::new(None),
            host: RwLock::new(HostSnapshot::default()),
        }
    }

    /// Replaces the gateway binding. The registry and client are rebuilt
    /// when `binding.generation` differs from the current one; returns
    /// whether they were. A repeated generation is a no-op, since the
    /// generation is the client's word that the gateway changed.
    pub fn set_gateway(&self, binding: GatewayBinding) -> bool {
        let generation = binding.generation;
        // The write lock is held across the check, the build, and the
        // store, and the watch is sent under it too: two concurrent pushes
        // with different generations then serialize, so the stored
        // resources and the watched generation always come from the same
        // caller. The build does no I/O, so holding the lock is cheap.
        // A poisoned lock holds a value written whole by a single store,
        // so it is intact and the poison is safe to clear.
        let mut current = self.gateway.write().unwrap_or_else(PoisonError::into_inner);
        if current
            .as_ref()
            .is_some_and(|resources| resources.generation() == generation)
        {
            return false;
        }
        *current = Some(Arc::new(GatewayResources::build(binding)));
        self.gateway_generation.send_replace(Some(generation));
        true
    }

    /// The resources of the most recently pushed gateway generation, or
    /// `None` before the first push.
    #[must_use]
    pub fn gateway(&self) -> Option<Arc<GatewayResources>> {
        self.gateway
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// A watch on the gateway generation.
    #[must_use]
    pub fn subscribe_gateway(&self) -> watch::Receiver<Option<u64>> {
        self.gateway_generation.subscribe()
    }

    /// Replaces the chat catalog binding and wakes the sessions watching
    /// its generation.
    pub fn set_catalog(&self, catalog: CatalogBinding) {
        let generation = catalog.generation;
        *self.catalog.write().unwrap_or_else(PoisonError::into_inner) = Some(catalog);
        self.catalog_generation.send_replace(Some(generation));
    }

    /// The most recently pushed catalog, or `None` before the first push.
    #[must_use]
    pub fn catalog(&self) -> Option<CatalogBinding> {
        self.catalog
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// A watch on the catalog generation.
    #[must_use]
    pub fn subscribe_catalog(&self) -> watch::Receiver<Option<u64>> {
        self.catalog_generation.subscribe()
    }

    /// Replaces the host snapshot; the next launch reads it.
    pub fn set_host(&self, host: HostSnapshot) {
        *self.host.write().unwrap_or_else(PoisonError::into_inner) = host;
    }

    /// The current host snapshot.
    #[must_use]
    pub fn host(&self) -> HostSnapshot {
        self.host
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Why launch-time model resolution cannot bind a descriptor. Each cause
/// becomes the launch error, reported to the operator instead of binding
/// a fabricated fallback descriptor.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CurrentModelError {
    /// The gateway's model catalog could not be fetched; the fetch
    /// failure is the source.
    #[error("the model catalog could not be fetched")]
    CatalogFetchFailed(#[source] CompletionError),
    /// The selected id is absent from the fetched catalog.
    #[error("the selected model `{0}` is absent from the fetched catalog")]
    SelectionAbsent(String),
}

/// Resolves the client's current model for one run's context. The
/// selection is read at launch, so a selection change takes effect on the
/// next run. A launch with no selection yet binds the retained catalog's
/// first chat-capable model, the same fallback a client's menu applies.
/// The typed descriptor comes from the gateway's model list through
/// [`fetch_model_catalog`].
///
/// Returns `Ok(None)` only when neither a selection nor a catalog model
/// exists, or the id is not representable; the prompt's declared roles
/// then stay unbound. A failed catalog fetch or a selection absent from
/// the fetched catalog is a reported [`CurrentModelError`], never a
/// fabricated fallback descriptor.
///
/// # Errors
/// Returns [`CurrentModelError::CatalogFetchFailed`] when the gateway's
/// model list cannot be fetched and [`CurrentModelError::SelectionAbsent`]
/// when the selected id is not in it.
pub async fn current_model(
    host: &HostSnapshot,
    catalog: Option<&CatalogBinding>,
    gateway: &GatewayBinding,
) -> Result<Option<ModelDescriptor>, CurrentModelError> {
    let Some(selected) = host.selected_model.clone().or_else(|| {
        catalog?
            .models
            .first()?
            .get("id")?
            .as_str()
            .map(str::to_owned)
    }) else {
        return Ok(None);
    };
    let id = match ModelId::gateway(&selected) {
        Ok(id) => id,
        Err(error) => {
            tracing::warn!(%error, "the selected model id is invalid");
            return Ok(None);
        }
    };
    let fetched = fetch_model_catalog(&gateway.api_root(), &gateway.key)
        .await
        .map_err(CurrentModelError::CatalogFetchFailed)?;
    let descriptor = fetched
        .get(&id)
        .cloned()
        .ok_or_else(|| CurrentModelError::SelectionAbsent(selected))?;
    Ok(Some(descriptor))
}

#[cfg(test)]
#[path = "environment-tests.rs"]
mod tests;
