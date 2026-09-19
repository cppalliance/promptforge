//! The harness handle: its configuration and the gateway binding a client
//! pushes across the door.

use std::fmt;
use std::path::PathBuf;
use std::sync::{PoisonError, RwLock};

/// What a client tells the harness at construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessConfig {
    /// The directory the harness discovers launchable agents in.
    pub agents_path: PathBuf,
    /// The directory the harness keeps its state under, the run log
    /// included.
    pub state_dir: PathBuf,
}

/// One generation of the gateway a client has bound the harness to.
///
/// The client calls [`Harness::set_gateway`] at startup and on every
/// gateway replacement; the harness rebuilds its capability registry and
/// model client when `generation` changes. The binding is data pushed
/// across the door: the harness never resolves a gateway itself.
#[derive(Clone, PartialEq, Eq)]
pub struct GatewayBinding {
    /// The gateway's base URL.
    pub base_url: String,
    /// The bearer key paired with `base_url`.
    pub key: String,
    /// Monotonic generation the client assigns to each replacement.
    pub generation: u64,
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

/// The harness: the engine's production host, seen from outside the
/// family.
///
/// One `Harness` serves every session a client launches, so the gateway
/// binding is replaceable through a shared reference; a client holds the
/// harness behind an `Arc` and calls [`Harness::set_gateway`] from
/// whichever task observes the replacement.
#[derive(Debug)]
pub struct Harness {
    config: HarnessConfig,
    gateway: RwLock<Option<GatewayBinding>>,
}

impl Harness {
    /// A harness over `config` with no gateway bound yet.
    #[must_use]
    pub fn new(config: HarnessConfig) -> Self {
        Self {
            config,
            gateway: RwLock::new(None),
        }
    }

    /// The configuration this harness was built with.
    #[must_use]
    pub fn config(&self) -> &HarnessConfig {
        &self.config
    }

    /// Replace the gateway binding; the latest call wins.
    pub fn set_gateway(&self, binding: GatewayBinding) {
        // A poisoned lock holds a binding written whole by a single store,
        // so the value is intact and the poison is safe to clear.
        *self.gateway.write().unwrap_or_else(PoisonError::into_inner) = Some(binding);
    }

    /// The most recently set gateway binding, or `None` before the first
    /// [`Harness::set_gateway`].
    #[must_use]
    pub fn gateway(&self) -> Option<GatewayBinding> {
        self.gateway
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
