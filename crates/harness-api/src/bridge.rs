//! Interim bridge: the facilities Workshop's session machinery still
//! reaches through the harness door before the session machinery itself
//! moves into the harness - the harness's model client, its capability
//! registry and activation, the implementation traits behind a run's
//! tools and input waits, and the session pieces that have already
//! moved (the supervisor reducer, the run lifecycle, agent discovery,
//! the input wait registry and its performer) -
//! re-exported here so that `workshop-sessions` names them through one
//! door today and the harness's own session runtime replaces them in
//! place as it lands.
//!
//! Pure indirection, plus [`first_party_registry`]: nothing else in this
//! module is defined here. Everything re-exported is already the
//! harness's (`harness-models`, `harness-capabilities`,
//! `harness-sessions`); the bridge and the registration function go once
//! `harness-sessions` builds the registry itself.

use std::sync::Arc;

/// The pure session pieces: the supervisor's state machine, the
/// synchronous run lifecycle that feeds it, agent discovery with the
/// embedded built-in chat, and the user-input wait registry with the
/// input performer over it.
pub use harness_sessions::{discovery, input, lifecycle, transition};

/// The performer trait behind a run's input waits and the boxed future
/// its implementations return.
pub use harness_runner::performers::{BoxFuture, InputPerformer};

/// The harness's gateway-facing model client and its failure vocabulary.
pub use harness_models::{
    CompletionError, CompletionErrorKind, GatewayClient, GatewayEndpoint, SecretString,
    fetch_model_catalog,
};

/// The capability registry and per-run activation: the activated tool
/// table and its result.
pub use harness_capabilities::{
    Activation, CapabilityRegistry, RegistryError, RegistryErrorKind, RunServices, ToolTable,
    activate,
};

/// The implementation trait behind a run's tool calls.
pub use harness_capabilities::Tool;

/// The first-party `promptforge/web` capability.
pub use harness_web::Web;

/// Builds a registry holding the first-party capabilities for one gateway
/// generation: today `promptforge/web`, built from the gateway's API root
/// (`root`, the OpenAI-shaped `/v1` base) and bearer `token`. The
/// registry is rebuilt when the gateway generation changes, so a
/// replacement gateway's root and key reach the contributed tools.
///
/// # Errors
/// Returns the web capability's own [`ToolError`] when `root` is not a
/// valid gateway API root or `token` is empty.
///
/// [`ToolError`]: promptforge_api_types::tools::ToolError
pub fn first_party_registry(
    root: &str,
    token: &str,
) -> Result<CapabilityRegistry, promptforge_api_types::tools::ToolError> {
    let web = Web::new(root, token)?;
    let mut registry = CapabilityRegistry::new();
    // A single registration cannot collide; the registry's error is
    // unreachable on this path, and dropping it keeps the signature to the
    // one failure a caller can act on.
    let _ = registry.register(Arc::new(web));
    Ok(registry)
}
