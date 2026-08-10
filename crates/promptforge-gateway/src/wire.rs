//! The OpenAI-shaped request and response bodies the gateway speaks.
//!
//! These are the gateway's own view of the wire contract. The executor defines
//! its own copies against the same JSON; the two are deliberately not shared,
//! because JSON is the contract and each side's struct is shaped by its role.
//! In v0 the message and choice payloads are kept as opaque JSON so everything
//! the gateway does not route passes through untouched.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::ThinkingMode;

/// An incoming chat completions request.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub(crate) struct ChatRequest {
    /// The model name, resolved against the routing table.
    pub model: String,
    /// The conversation messages, passed through to the backend verbatim.
    pub messages: Vec<Value>,
    /// Every field the gateway does not name, preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// An outgoing chat completions response.
#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub(crate) struct ChatResponse {
    /// The model name, rewritten to the caller's requested name.
    pub model: String,
    /// The completion choices, passed through from the backend verbatim.
    pub choices: Vec<Value>,
    /// Every field the gateway does not name, preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// The OpenAI-shaped model list returned by `GET /v1/models`.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub(crate) struct ModelsResponse {
    /// Always `"list"`.
    pub object: &'static str,
    /// One entry per configured `[[model]]`, in config order.
    pub data: Vec<ModelInfo>,
}

/// One catalogued model, with PromptForge extensions beside the OpenAI `id`.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub(crate) struct ModelInfo {
    /// The caller-facing model name (`[[model]].name`).
    pub id: String,
    /// Always `"model"`.
    pub object: &'static str,
    /// Prose describing the model for catalog consumers and semantic bind.
    pub description: String,
    /// Context window size in tokens.
    pub context: u32,
    /// Whether thinking tokens are never, always, or switchably available.
    pub thinking: ThinkingMode,
    /// The tool-calling dialect used by this model (`"openai"`, `"gemma3_tool_code"`).
    pub tool_dialect: String,
    /// Whether tool calls are handled natively or emulated (`"native"`, `"emulated"`).
    pub tools_mode: String,
}
