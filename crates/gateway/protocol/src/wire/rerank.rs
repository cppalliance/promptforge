//! The rerank request and response bodies and their trust-boundary validation.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// An incoming rerank request (the llama-server/vLLM/Jina shape: a query and
/// a document set in, ranked relevance scores out).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct RerankRequest {
    /// The model name, resolved against the routing table.
    pub model: String,
    /// The query each document is scored against.
    pub query: String,
    /// The candidate documents to rank.
    pub documents: Vec<String>,
    /// How many top-ranked results to return; absent means the backend's
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_n: Option<u32>,
    /// Every field the gateway does not name, preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl RerankRequest {
    /// Reserved top-level keys that must never appear in the passthrough `rest`.
    const RESERVED: [&'static str; 4] = ["model", "query", "documents", "top_n"];

    /// Validates the request shape at the trust boundary, without coercion.
    ///
    /// Rejects an empty model, an empty query, an empty document set, and any
    /// reserved key smuggled into the flattened `rest` map (WIRE-001/003).
    /// Everything else passes through verbatim.
    ///
    /// # Errors
    /// Returns a static reason string when the model or query is empty, the
    /// document set is empty, or `rest` collides with a named field.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.model.trim().is_empty() {
            return Err("model must not be empty");
        }
        if self.query.trim().is_empty() {
            return Err("query must not be empty");
        }
        if self.documents.is_empty() {
            return Err("documents must not be empty");
        }
        if Self::RESERVED
            .iter()
            .any(|key| self.rest.contains_key(*key))
        {
            return Err("rest must not contain a reserved key (model, query, documents, top_n)");
        }
        Ok(())
    }
}

/// An outgoing rerank response.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RerankResponse {
    /// The model name, rewritten to the caller's requested name.
    pub model: String,
    /// The ranked results, passed through from the backend verbatim.
    pub results: Vec<Value>,
    /// Every field the gateway does not name (for example `usage`), preserved
    /// verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl RerankResponse {
    /// Reserved top-level keys that must never appear in the passthrough `rest`.
    const RESERVED: [&'static str; 2] = ["model", "results"];

    /// Validates the upstream response shape, treating structural failure as an
    /// upstream-protocol error rather than silently passing it through.
    ///
    /// Each result must pass the minimal shape check: an `index` and a
    /// `relevance_score` (WIRE-002). Every other field (for example a Jina
    /// `document` echo) passes through untouched.
    ///
    /// # Errors
    /// Returns a static reason string when a result fails the minimal shape
    /// check or a reserved key collides with the flattened `rest` map.
    pub fn validate(&self) -> Result<(), &'static str> {
        for result in &self.results {
            let object = result
                .as_object()
                .ok_or("upstream returned a non-object rerank result")?;
            if !object.contains_key("index") {
                return Err("upstream rerank result is missing index");
            }
            if !object.contains_key("relevance_score") {
                return Err("upstream rerank result is missing relevance_score");
            }
        }
        if Self::RESERVED
            .iter()
            .any(|key| self.rest.contains_key(*key))
        {
            return Err("rest must not contain a reserved key (model, results)");
        }
        Ok(())
    }
}
