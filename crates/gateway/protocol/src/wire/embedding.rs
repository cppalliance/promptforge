//! The embeddings request and response bodies and their trust-boundary validation.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The text to embed: one string or a batch of strings (OpenAI shape).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum EmbeddingInput {
    /// A single input string.
    One(String),
    /// A batch of input strings.
    Many(Vec<String>),
}

/// An incoming embeddings request.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct EmbeddingRequest {
    /// The model name, resolved against the routing table.
    pub model: String,
    /// The text to embed.
    pub input: EmbeddingInput,
    /// The encoding format (`"float"` or `"base64"`); absent means the
    /// backend's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_format: Option<String>,
    /// Every field the gateway does not name, preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl EmbeddingRequest {
    /// Reserved top-level keys that must never appear in the passthrough `rest`.
    const RESERVED: [&'static str; 3] = ["model", "input", "encoding_format"];

    /// Validates the request shape at the trust boundary, without coercion.
    ///
    /// Rejects an empty model, an empty input batch, and any reserved key
    /// smuggled into the flattened `rest` map (WIRE-001/003). Everything else
    /// passes through verbatim.
    ///
    /// # Errors
    /// Returns a static reason string when the model is empty, the input batch
    /// is empty, or `rest` collides with a named field.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.model.trim().is_empty() {
            return Err("model must not be empty");
        }
        if matches!(&self.input, EmbeddingInput::Many(batch) if batch.is_empty()) {
            return Err("input must not be an empty batch");
        }
        if Self::RESERVED
            .iter()
            .any(|key| self.rest.contains_key(*key))
        {
            return Err("rest must not contain a reserved key (model, input, encoding_format)");
        }
        Ok(())
    }
}

/// An outgoing embeddings response.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct EmbeddingResponse {
    /// The model name, rewritten to the caller's requested name.
    pub model: String,
    /// The embedding entries, passed through from the backend verbatim.
    pub data: Vec<Value>,
    /// Every field the gateway does not name (for example `usage`), preserved
    /// verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl EmbeddingResponse {
    /// Reserved top-level keys that must never appear in the passthrough `rest`.
    const RESERVED: [&'static str; 2] = ["model", "data"];

    /// Validates the upstream response shape, treating structural failure as an
    /// upstream-protocol error rather than silently passing it through.
    ///
    /// Each entry must pass the minimal shape check: an `embedding` and an
    /// `index` (WIRE-002). Every other field passes through untouched.
    ///
    /// # Errors
    /// Returns a static reason string when an entry fails the minimal shape
    /// check or a reserved key collides with the flattened `rest` map.
    pub fn validate(&self) -> Result<(), &'static str> {
        for entry in &self.data {
            let object = entry
                .as_object()
                .ok_or("upstream returned a non-object embedding entry")?;
            if !object.contains_key("embedding") {
                return Err("upstream embedding entry is missing embedding");
            }
            if !object.contains_key("index") {
                return Err("upstream embedding entry is missing index");
            }
        }
        if Self::RESERVED
            .iter()
            .any(|key| self.rest.contains_key(*key))
        {
            return Err("rest must not contain a reserved key (model, data)");
        }
        Ok(())
    }
}
