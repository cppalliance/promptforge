//! The OpenAI-shaped request and response bodies the gateway speaks.
//!
//! These are the gateway's own view of the wire contract. The executor defines
//! its own copies against the same JSON; the two are deliberately not shared,
//! because JSON is the contract and each side's struct is shaped by its role.
//! In v0 the message and choice payloads are kept as opaque JSON so everything
//! the gateway does not route passes through untouched.
//!
//! WIRE-005: the `object` discriminators are fixed `&'static str` literals
//! (`"list"`, `"model"`), so they are already closed. The `tool_dialect` and
//! `tools_mode` catalog fields stay `String`: they are registry-assigned open
//! identifiers owned by `promptforge-core` (see the ROUTING-005 disposition),
//! stringified only at this catalog boundary rather than re-modeled as a closed
//! gateway enum that would fight core's vocabulary.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::ThinkingMode;

/// An incoming chat completions request.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
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

impl ChatRequest {
    /// Reserved top-level keys that must never appear in the passthrough `rest`.
    const RESERVED: [&'static str; 2] = ["model", "messages"];

    /// Validate the request shape at the trust boundary, without coercion.
    ///
    /// Rejects an empty model, any message that is not a JSON object, and any
    /// reserved key smuggled into the flattened `rest` map (WIRE-003); the
    /// message object contents themselves pass through verbatim.
    ///
    /// # Errors
    /// Returns a static reason string when the model is empty, a message is not
    /// a JSON object, or `rest` collides with a named field.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.model.trim().is_empty() {
            return Err("model must not be empty");
        }
        if self.messages.iter().any(|message| !message.is_object()) {
            return Err("each message must be a JSON object");
        }
        if Self::RESERVED
            .iter()
            .any(|key| self.rest.contains_key(*key))
        {
            return Err("rest must not contain a reserved key (model, messages)");
        }
        Ok(())
    }
}

/// An outgoing chat completions response.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
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

impl ChatResponse {
    /// Reserved top-level keys that must never appear in the passthrough `rest`.
    const RESERVED: [&'static str; 2] = ["model", "choices"];

    /// Validate the upstream response shape, treating structural failure as an
    /// upstream-protocol error rather than silently passing it through.
    ///
    /// # Errors
    /// Returns a static reason string when a choice is not a JSON object or a
    /// reserved key collides with the flattened `rest` map (WIRE-003).
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.choices.iter().any(|choice| !choice.is_object()) {
            return Err("upstream returned a non-object choice");
        }
        if Self::RESERVED
            .iter()
            .any(|key| self.rest.contains_key(*key))
        {
            return Err("rest must not contain a reserved key (model, choices)");
        }
        Ok(())
    }
}

/// The OpenAI-shaped model list returned by `GET /v1/models`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub(crate) struct ModelsResponse {
    /// Always `"list"`.
    pub object: &'static str,
    /// One entry per configured `[[model]]`, in config order.
    pub data: Vec<ModelInfo>,
}

/// One catalogued model, with PromptForge extensions beside the OpenAI `id`.
#[derive(Clone, Debug, PartialEq, Serialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn request(model: &str, messages: Vec<Value>) -> ChatRequest {
        ChatRequest {
            model: model.to_owned(),
            messages,
            rest: Map::new(),
        }
    }

    #[test]
    fn accepts_object_messages() {
        let req = request(
            "m",
            vec![serde_json::json!({ "role": "user", "content": "hi" })],
        );
        assert!(req.validate().is_ok());
    }

    #[test]
    fn rejects_empty_model_and_non_object_messages() {
        assert!(request("  ", vec![]).validate().is_err());
        assert!(
            request("m", vec![serde_json::json!("not-an-object")])
                .validate()
                .is_err()
        );
    }

    #[test]
    fn request_rejects_reserved_keys_in_rest() {
        let mut req = request("m", vec![]);
        req.rest
            .insert("messages".to_owned(), serde_json::json!(["x"]));
        assert!(req.validate().is_err());
    }

    #[test]
    fn response_rejects_reserved_keys_in_rest() {
        let mut response = ChatResponse {
            model: "m".to_owned(),
            choices: vec![],
            rest: Map::new(),
        };
        response
            .rest
            .insert("choices".to_owned(), serde_json::json!([]));
        assert!(response.validate().is_err());
    }

    #[test]
    fn response_rejects_non_object_choice() {
        let response = ChatResponse {
            model: "m".to_owned(),
            choices: vec![serde_json::json!(42)],
            rest: Map::new(),
        };
        assert!(response.validate().is_err());
    }

    #[test]
    fn request_round_trips_through_json() {
        let json = serde_json::json!({
            "model": "m",
            "messages": [{ "role": "user", "content": "hi" }],
            "temperature": 0.5,
            "stream": false,
        });
        let req: ChatRequest = serde_json::from_value(json.clone()).expect("parse request");
        // Unnamed fields land in `rest`, not on named fields.
        assert!(req.rest.contains_key("temperature"));
        assert!(req.rest.contains_key("stream"));
        assert!(!req.rest.contains_key("model"));
        assert!(!req.rest.contains_key("messages"));
        // Serialize back and re-parse: the value is stable.
        let reparsed: ChatRequest =
            serde_json::from_value(serde_json::to_value(&req).expect("serialize"))
                .expect("reparse");
        assert_eq!(req, reparsed);
    }

    #[test]
    fn response_round_trips_and_preserves_unknown_fields() {
        let json = serde_json::json!({
            "model": "backend",
            "choices": [{ "index": 0 }],
            "usage": { "total_tokens": 7 },
        });
        let resp: ChatResponse = serde_json::from_value(json).expect("parse response");
        assert!(resp.rest.contains_key("usage"));
        let reparsed: ChatResponse =
            serde_json::from_value(serde_json::to_value(&resp).expect("serialize"))
                .expect("reparse");
        assert_eq!(resp, reparsed);
    }
}
