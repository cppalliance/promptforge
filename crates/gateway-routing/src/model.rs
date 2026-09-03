//! Routing table entries: the [`Model`] and [`Endpoint`] vocabulary shared by
//! the gateway's routing table and the local inference crate.

use std::sync::Arc;

use gateway_config::{Capabilities, ModelKind, ThinkingMode};
use gateway_protocol::upstream::Upstream;

use crate::queue::DominionQueue;

/// The `tool_dialect` value selecting the emulated Gemma3 `tool_code`
/// content-fence dialect.
///
/// This is vocabulary for [`Model::tool_dialect`]: the local inference
/// crate's dialect probing resolves it from child evidence, and the gateway's
/// dialect emulation matches on it, so both sides name one constant.
pub const GEMMA3_TOOL_CODE: &str = "gemma3_tool_code";

/// One backend endpoint plus the upstream that talks to it.
pub struct Endpoint {
    /// The endpoint's configured id.
    pub id: String,
    /// The upstream implementation forwarding to this backend.
    pub upstream: Arc<dyn Upstream>,
    /// Admission control: concurrency limit plus bounded waiting queue.
    /// Endpoints bound to the same dominion hold clones of one shared queue
    /// and compete for a single pool of slots; an endpoint with no dominion
    /// is unlimited.
    pub queue: DominionQueue,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("id", &self.id)
            .field("queue", &self.queue)
            .finish_non_exhaustive()
    }
}

/// One model, resolved to a backend endpoint and the backend's model string.
#[derive(Debug)]
pub struct Model {
    /// The caller-facing model name.
    pub name: String,
    /// The workload this model serves: chat, embedding, or classifier.
    pub kind: ModelKind,
    /// Prose describing the model for catalog consumers.
    pub description: String,
    /// Context window size in tokens.
    pub context: u32,
    /// Whether thinking tokens are never, always, or switchably available.
    pub thinking: ThinkingMode,
    /// Capability metadata advertised on the catalog.
    pub capabilities: Capabilities,
    /// The tool-calling dialect used by this model (e.g. `"openai"`, `"gemma3_tool_code"`).
    pub tool_dialect: String,
    /// The string the backend knows this model by.
    pub upstream_name: String,
    /// The endpoint serving this model (v0 uses the first configured one).
    pub endpoint: Arc<Endpoint>,
}
