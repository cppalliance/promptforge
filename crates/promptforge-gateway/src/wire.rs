//! The OpenAI-shaped request and response bodies the gateway speaks.
//!
//! These are the gateway's own view of the wire contract. The executor defines
//! its own copies against the same JSON; the two are deliberately not shared,
//! because JSON is the contract and each side's struct is shaped by its role.
//! In v0 the message and choice payloads are kept as opaque JSON so everything
//! the gateway does not route passes through untouched.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// An incoming chat completions request.
#[derive(Debug, Deserialize, Serialize)]
pub struct ChatRequest {
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
pub struct ChatResponse {
    /// The model name, rewritten to the caller's requested name.
    pub model: String,
    /// The completion choices, passed through from the backend verbatim.
    pub choices: Vec<Value>,
    /// Every field the gateway does not name, preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}
