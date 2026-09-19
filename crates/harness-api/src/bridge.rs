//! Interim bridge: the engine facilities Workshop's session machinery
//! still reaches through the `promptforge-api-runtime` door - the model
//! client, the capability registry, and activation - re-exported here so
//! that `workshop-sessions` names them through the harness door today and
//! the harness's own implementations replace them in place as they land.
//!
//! Pure indirection: nothing in this module is defined here. The
//! re-exports are temporary and are removed once the session machinery
//! has moved into the harness family and the engine no longer owns a
//! client or a registry.

/// The gateway-facing model client and its failure vocabulary.
pub use promptforge_api_runtime::client::{
    CompletionError, CompletionErrorKind, GatewayClient, GatewayEndpoint, SecretString,
    fetch_model_catalog,
};

/// The capability registry and the first-party `promptforge/web` capability.
pub use promptforge_api_runtime::{CapabilityRegistry, RegistryError, RegistryErrorKind, Web};

/// Per-run capability activation: the activated tool table and its result.
pub use promptforge_api_runtime::{Activation, ToolTable, activate};
