//! Interim bridge: the facilities Workshop's session machinery still
//! reaches through the harness door before the session machinery itself
//! moves into the harness - the harness's model client, and the engine's
//! capability registry and activation - re-exported here so that
//! `workshop-sessions` names them through one door today and the harness's
//! own implementations replace them in place as they land.
//!
//! Pure indirection: nothing in this module is defined here. The model
//! client is already the harness's (`harness-models`); the registry and
//! activation re-exports are temporary and are removed once the engine no
//! longer owns a registry.

/// The harness's gateway-facing model client and its failure vocabulary.
pub use harness_models::{
    CompletionError, CompletionErrorKind, GatewayClient, GatewayEndpoint, SecretString,
    fetch_model_catalog,
};

/// The capability registry and the first-party `promptforge/web` capability.
pub use promptforge_api_runtime::{CapabilityRegistry, RegistryError, RegistryErrorKind, Web};

/// Per-run capability activation: the activated tool table and its result.
pub use promptforge_api_runtime::{Activation, ToolTable, activate};
