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

use promptforge_gateway_config::ThinkingMode;

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

/// Chat roles the gateway recognizes at the request boundary (OpenAI set).
const SUPPORTED_ROLES: [&str; 6] = [
    "system",
    "user",
    "assistant",
    "tool",
    "function",
    "developer",
];

impl ChatRequest {
    /// Reserved top-level keys that must never appear in the passthrough `rest`.
    const RESERVED: [&'static str; 2] = ["model", "messages"];

    /// Validate the request shape at the trust boundary, without coercion.
    ///
    /// Rejects an empty model, an empty `messages` array, any message that is
    /// not a minimally-shaped chat message (an object with a supported string
    /// `role` and either `content` or a tool/function call), and any reserved
    /// key smuggled into the flattened `rest` map (WIRE-001/003). Everything
    /// else in each message object passes through verbatim.
    ///
    /// # Errors
    /// Returns a static reason string when the model is empty, `messages` is
    /// empty, a message fails the minimal shape check, or `rest` collides with a
    /// named field.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.model.trim().is_empty() {
            return Err("model must not be empty");
        }
        if self.messages.is_empty() {
            return Err("messages must not be empty");
        }
        for message in &self.messages {
            validate_message(message)?;
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

/// Validate one chat message's minimal shape without reconstructing it (WIRE-001).
///
/// A message must be a JSON object with a supported string `role` and must carry
/// either `content` (any shape: string, array, or null) or a tool/function call.
/// Unknown fields are left untouched for verbatim passthrough.
fn validate_message(message: &Value) -> Result<(), &'static str> {
    let object = message
        .as_object()
        .ok_or("each message must be a JSON object")?;
    let role = object
        .get("role")
        .and_then(Value::as_str)
        .ok_or("each message must have a string role")?;
    if !SUPPORTED_ROLES.contains(&role) {
        return Err("each message role is not supported");
    }
    let has_content = object.contains_key("content");
    let has_call = object.contains_key("tool_calls") || object.contains_key("function_call");
    if !has_content && !has_call {
        return Err("each message must carry content or a tool/function call");
    }
    Ok(())
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
    /// Each choice must be a minimally-shaped object: an `index` plus one of the
    /// supported payloads (`message`, `delta`, or `text`). This rejects a
    /// backend that returns a success status with a structurally broken body
    /// (WIRE-002) while leaving every other field untouched for passthrough.
    ///
    /// # Errors
    /// Returns a static reason string when a choice is not a minimally-shaped
    /// object or a reserved key collides with the flattened `rest` map.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        for choice in &self.choices {
            validate_choice(choice)?;
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

/// Validate one response choice's minimal shape (WIRE-002).
///
/// A choice must be a JSON object carrying an `index` and one of the supported
/// payload fields (`message` for non-streaming, `delta` for streaming, or the
/// legacy `text`). Extra fields (for example `finish_reason`, `logprobs`) pass
/// through untouched.
fn validate_choice(choice: &Value) -> Result<(), &'static str> {
    let object = choice
        .as_object()
        .ok_or("upstream returned a non-object choice")?;
    if !object.contains_key("index") {
        return Err("upstream choice is missing index");
    }
    let has_payload = object.contains_key("message")
        || object.contains_key("delta")
        || object.contains_key("text");
    if !has_payload {
        return Err("upstream choice is missing message/delta/text");
    }
    Ok(())
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
        let mut req = request(
            "m",
            vec![serde_json::json!({ "role": "user", "content": "hi" })],
        );
        req.rest
            .insert("messages".to_owned(), serde_json::json!(["x"]));
        assert!(req.validate().is_err());
    }

    #[test]
    fn rejects_empty_messages_array() {
        // WIRE-001: an empty conversation is not a valid chat request.
        assert!(request("m", vec![]).validate().is_err());
    }

    #[test]
    fn rejects_message_without_role_or_content() {
        // WIRE-001: a message object still needs a supported role and a payload.
        assert!(
            request("m", vec![serde_json::json!({ "content": "hi" })])
                .validate()
                .is_err()
        );
        assert!(
            request("m", vec![serde_json::json!({ "role": "user" })])
                .validate()
                .is_err()
        );
        assert!(
            request(
                "m",
                vec![serde_json::json!({ "role": "spork", "content": "x" })]
            )
            .validate()
            .is_err()
        );
    }

    #[test]
    fn accepts_assistant_tool_call_without_content() {
        // WIRE-001: an assistant tool-call message legitimately omits content.
        let req = request(
            "m",
            vec![serde_json::json!({
                "role": "assistant",
                "tool_calls": [{ "id": "1", "type": "function" }]
            })],
        );
        assert!(req.validate().is_ok());
    }

    #[test]
    fn response_rejects_choice_missing_index_or_payload() {
        // WIRE-002: a structurally broken choice is an upstream-protocol failure.
        let missing_index = ChatResponse {
            model: "m".to_owned(),
            choices: vec![serde_json::json!({ "message": { "role": "assistant" } })],
            rest: Map::new(),
        };
        assert!(missing_index.validate().is_err());
        let missing_payload = ChatResponse {
            model: "m".to_owned(),
            choices: vec![serde_json::json!({ "index": 0 })],
            rest: Map::new(),
        };
        assert!(missing_payload.validate().is_err());
    }

    #[test]
    fn response_accepts_minimally_shaped_choice() {
        let response = ChatResponse {
            model: "m".to_owned(),
            choices: vec![serde_json::json!({
                "index": 0,
                "message": { "role": "assistant", "content": "hi" },
                "finish_reason": "stop"
            })],
            rest: Map::new(),
        };
        assert!(response.validate().is_ok());
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
