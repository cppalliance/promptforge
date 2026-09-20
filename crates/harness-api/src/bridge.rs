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
//! Pure indirection: nothing in this module is defined here. Everything
//! re-exported is already the harness's (`harness-models`,
//! `harness-capabilities`, `harness-sessions`); the bridge goes once
//! Workshop opens its sessions through [`Harness`](crate::Harness).

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

/// Builds a registry holding the first-party capabilities for one gateway
/// generation; `harness-sessions` owns registration.
pub use harness_sessions::environment::first_party_registry;
