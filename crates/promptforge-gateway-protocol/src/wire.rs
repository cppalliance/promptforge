//! The OpenAI-shaped request and response bodies the gateway speaks.
//!
//! These are the gateway's own view of the wire contract. The executor defines
//! its own copies against the same JSON; the two are deliberately not shared,
//! because JSON is the contract and each side's struct is shaped by its role.
//! In v0 the message and choice payloads are kept as opaque JSON so everything
//! the gateway does not route passes through untouched.
//!
//! WIRE-005: the `object` discriminators are fixed `&'static str` literals
//! (`"list"`, `"model"`), so they are already closed.
//!
//! `gateway_warning` is a gateway-specific extension on the OpenAI response
//! shape: when an emulated tool dialect recovers from a malformed tool fence,
//! the affected choice's message carries the reason under `gateway_warning`
//! next to its emptied `content`. Downstream serde ignores the unknown field.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use promptforge_gateway_config::{Capabilities, ModelKind, ThinkingMode};

/// An incoming chat completions request.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ChatRequest {
    /// The model name, resolved against the routing table.
    pub model: String,
    /// The conversation messages, passed through to the backend verbatim.
    pub messages: Vec<Value>,
    /// Whether the caller asked for a streaming (SSE) completion. Absent
    /// means non-streaming; an absent `stream` is never forwarded.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stream: bool,
    /// Every field the gateway does not name, preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// serde `skip_serializing_if` predicate for the `stream` flag.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if predicates receive the field by reference"
)]
fn is_false(value: &bool) -> bool {
    !*value
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
    const RESERVED: [&'static str; 3] = ["model", "messages", "stream"];

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
    pub fn validate(&self) -> Result<(), &'static str> {
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
            return Err("rest must not contain a reserved key (model, messages, stream)");
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
pub struct ChatResponse {
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
    pub fn validate(&self) -> Result<(), &'static str> {
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

/// One chunk of a streaming chat completion (OpenAI streaming shape).
///
/// A `stream: true` completion arrives as a sequence of these chunks, each
/// carrying partial `delta` content instead of a complete `message`. The
/// terminal `[DONE]` sentinel is not JSON and never deserializes into this
/// type; the relay special-cases it before parsing.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ChatChunk {
    /// The model name, rewritten to the caller's requested name.
    pub model: String,
    /// The partial choices for this chunk.
    pub choices: Vec<ChatChunkChoice>,
    /// Every field the gateway does not name (for example `usage` on a
    /// final chunk), preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl ChatChunk {
    /// Validate one upstream chunk's minimal shape before it is relayed.
    ///
    /// A chunk must carry at least one choice; each choice's `index` and
    /// `delta` are required typed fields, so deserialization has already
    /// proven them present. A chunk that fails this check (for example a
    /// usage-only summary object a backend appends mid-stream) is malformed:
    /// the parser logs and skips it rather than relaying it or ending the
    /// stream.
    ///
    /// # Errors
    /// Returns a static reason string when the chunk carries no choices.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.choices.is_empty() {
            return Err("upstream chunk has no choices");
        }
        Ok(())
    }
}

/// One partial choice in a [`ChatChunk`]: an `index` plus a `delta` carrying
/// the incremental payload (`role` on the first chunk, content or tool-call
/// fragments thereafter).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ChatChunkChoice {
    /// The completion choice this delta belongs to.
    pub index: u32,
    /// The incremental payload, kept as opaque JSON so every field the
    /// gateway does not route passes through untouched.
    pub delta: Value,
    /// Every field the gateway does not name (for example `finish_reason`
    /// on the terminal chunk), preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// The text to embed: one string or a batch of strings (OpenAI shape).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
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

    /// Validate the request shape at the trust boundary, without coercion.
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

    /// Validate the upstream response shape, treating structural failure as an
    /// upstream-protocol error rather than silently passing it through.
    ///
    /// Each entry must be a minimally-shaped object carrying an `embedding`
    /// and an `index` (WIRE-002). Every other field passes through untouched.
    ///
    /// # Errors
    /// Returns a static reason string when an entry is not a minimally-shaped
    /// object or a reserved key collides with the flattened `rest` map.
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

    /// Validate the request shape at the trust boundary, without coercion.
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

    /// Validate the upstream response shape, treating structural failure as an
    /// upstream-protocol error rather than silently passing it through.
    ///
    /// Each result must be a minimally-shaped object carrying an `index` and a
    /// `relevance_score` (WIRE-002). Every other field (for example a Jina
    /// `document` echo) passes through untouched.
    ///
    /// # Errors
    /// Returns a static reason string when a result is not a minimally-shaped
    /// object or a reserved key collides with the flattened `rest` map.
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

/// The OpenAI-shaped model list returned by `GET /v1/models`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelsResponse {
    /// Always `"list"`.
    pub object: &'static str,
    /// One entry per configured `[[model]]`, in config order.
    pub data: Vec<ModelInfo>,
}

/// One catalogued model, with PromptForge extensions beside the OpenAI `id`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelInfo {
    /// The caller-facing model name (`[[model]].name`).
    pub id: String,
    /// Always `"model"`.
    pub object: &'static str,
    /// The workload this model serves (`"chat"`, `"embedding"`, `"classifier"`).
    pub kind: ModelKind,
    /// Prose describing the model for catalog consumers and semantic bind.
    pub description: String,
    /// Context window size in tokens.
    pub context: u32,
    /// Whether thinking tokens are never, always, or switchably available.
    pub thinking: ThinkingMode,
    /// Capability metadata (`max_output`, `images`, effort levels, and so
    /// on), flattened into the catalog entry.
    #[serde(flatten)]
    pub capabilities: Capabilities,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(model: &str, messages: Vec<Value>) -> ChatRequest {
        ChatRequest {
            model: model.to_owned(),
            messages,
            stream: false,
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
        assert!(!req.stream);
        assert!(!req.rest.contains_key("stream"));
        assert!(!req.rest.contains_key("model"));
        assert!(!req.rest.contains_key("messages"));
        // Serialize back and re-parse: the value is stable.
        let reparsed: ChatRequest =
            serde_json::from_value(serde_json::to_value(&req).expect("serialize"))
                .expect("reparse");
        assert_eq!(req, reparsed);
    }

    #[test]
    fn request_stream_flag_round_trips_and_omits_when_absent() {
        let json = serde_json::json!({
            "model": "m",
            "messages": [{ "role": "user", "content": "hi" }],
            "stream": true,
        });
        let req: ChatRequest = serde_json::from_value(json).expect("parse request");
        assert!(req.stream);
        assert_eq!(
            serde_json::to_value(&req).expect("serialize").get("stream"),
            Some(&serde_json::json!(true))
        );
        // An absent stream flag neither errors nor serializes as false.
        let req = request(
            "m",
            vec![serde_json::json!({ "role": "user", "content": "hi" })],
        );
        assert!(!req.stream);
        assert!(
            !serde_json::to_value(&req)
                .expect("serialize")
                .as_object()
                .expect("object")
                .contains_key("stream")
        );
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

    #[test]
    fn chat_chunk_round_trips_and_preserves_unknown_fields() {
        let json = serde_json::json!({
            "id": "chatcmpl-1",
            "object": "chat.completion.chunk",
            "model": "backend",
            "choices": [
                { "index": 0, "delta": { "role": "assistant", "content": "Hel" }, "finish_reason": null }
            ],
        });
        let chunk: ChatChunk = serde_json::from_value(json).expect("parse chunk");
        assert_eq!(chunk.model, "backend");
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(chunk.choices[0].index, 0);
        assert_eq!(
            chunk.choices[0]
                .delta
                .get("content")
                .and_then(Value::as_str),
            Some("Hel")
        );
        // Unnamed fields land in `rest`, not on named fields.
        assert!(chunk.rest.contains_key("id"));
        assert!(chunk.choices[0].rest.contains_key("finish_reason"));
        assert!(!chunk.rest.contains_key("model"));
        assert!(!chunk.rest.contains_key("choices"));
        let reparsed: ChatChunk =
            serde_json::from_value(serde_json::to_value(&chunk).expect("serialize"))
                .expect("reparse");
        assert_eq!(chunk, reparsed);
    }

    #[test]
    fn chat_chunk_rejects_empty_choices() {
        // A chunk with no choices (for example a usage-only summary object)
        // is malformed: logged and skipped, never relayed.
        let chunk = ChatChunk {
            model: "m".to_owned(),
            choices: vec![],
            rest: Map::new(),
        };
        assert!(chunk.validate().is_err());
    }

    #[test]
    fn chat_chunk_round_trips_an_empty_delta() {
        // The terminal chunk legitimately carries an empty delta plus a
        // finish_reason; it must survive the round-trip.
        let json = serde_json::json!({
            "model": "backend",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
        });
        let chunk: ChatChunk = serde_json::from_value(json).expect("parse chunk");
        assert_eq!(
            chunk.choices[0]
                .rest
                .get("finish_reason")
                .and_then(Value::as_str),
            Some("stop")
        );
        let reparsed: ChatChunk =
            serde_json::from_value(serde_json::to_value(&chunk).expect("serialize"))
                .expect("reparse");
        assert_eq!(chunk, reparsed);
    }

    fn embedding_request(model: &str, input: EmbeddingInput) -> EmbeddingRequest {
        EmbeddingRequest {
            model: model.to_owned(),
            input,
            encoding_format: None,
            rest: Map::new(),
        }
    }

    #[test]
    fn embedding_request_round_trips_with_string_input() {
        let json = serde_json::json!({
            "model": "m",
            "input": "embed me",
            "encoding_format": "base64",
            "dimensions": 512,
        });
        let req: EmbeddingRequest = serde_json::from_value(json).expect("parse request");
        assert_eq!(req.input, EmbeddingInput::One("embed me".to_owned()));
        assert_eq!(req.encoding_format.as_deref(), Some("base64"));
        // Unnamed fields land in `rest`, not on named fields.
        assert!(req.rest.contains_key("dimensions"));
        assert!(!req.rest.contains_key("model"));
        assert!(!req.rest.contains_key("input"));
        assert!(!req.rest.contains_key("encoding_format"));
        let reparsed: EmbeddingRequest =
            serde_json::from_value(serde_json::to_value(&req).expect("serialize"))
                .expect("reparse");
        assert_eq!(req, reparsed);
    }

    #[test]
    fn embedding_request_round_trips_with_array_input() {
        let json = serde_json::json!({
            "model": "m",
            "input": ["one", "two"],
        });
        let req: EmbeddingRequest = serde_json::from_value(json).expect("parse request");
        assert_eq!(
            req.input,
            EmbeddingInput::Many(vec!["one".to_owned(), "two".to_owned()])
        );
        // An absent encoding_format neither errors nor serializes as null.
        assert_eq!(req.encoding_format, None);
        assert!(
            !serde_json::to_value(&req)
                .expect("serialize")
                .as_object()
                .expect("object")
                .contains_key("encoding_format")
        );
        let reparsed: EmbeddingRequest =
            serde_json::from_value(serde_json::to_value(&req).expect("serialize"))
                .expect("reparse");
        assert_eq!(req, reparsed);
    }

    #[test]
    fn embedding_request_rejects_empty_model_and_empty_batch() {
        assert!(
            embedding_request("  ", EmbeddingInput::One("x".to_owned()))
                .validate()
                .is_err()
        );
        assert!(
            embedding_request("m", EmbeddingInput::Many(vec![]))
                .validate()
                .is_err()
        );
        assert!(
            embedding_request("m", EmbeddingInput::One("x".to_owned()))
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn embedding_request_rejects_reserved_keys_in_rest() {
        let mut req = embedding_request("m", EmbeddingInput::One("x".to_owned()));
        req.rest.insert("input".to_owned(), serde_json::json!("y"));
        assert!(req.validate().is_err());
    }

    #[test]
    fn embedding_response_round_trips_and_preserves_usage() {
        let json = serde_json::json!({
            "object": "list",
            "model": "backend",
            "data": [{ "object": "embedding", "index": 0, "embedding": [0.1, 0.2] }],
            "usage": { "prompt_tokens": 3, "total_tokens": 3 },
        });
        let resp: EmbeddingResponse = serde_json::from_value(json).expect("parse response");
        assert!(resp.validate().is_ok());
        assert!(resp.rest.contains_key("usage"));
        let reparsed: EmbeddingResponse =
            serde_json::from_value(serde_json::to_value(&resp).expect("serialize"))
                .expect("reparse");
        assert_eq!(resp, reparsed);
    }

    #[test]
    fn embedding_response_rejects_malformed_entries() {
        // WIRE-002: a structurally broken entry is an upstream-protocol failure.
        let response = |entry: Value| EmbeddingResponse {
            model: "m".to_owned(),
            data: vec![entry],
            rest: Map::new(),
        };
        assert!(response(serde_json::json!(42)).validate().is_err());
        assert!(
            response(serde_json::json!({ "index": 0 }))
                .validate()
                .is_err()
        );
        assert!(
            response(serde_json::json!({ "embedding": [0.1] }))
                .validate()
                .is_err()
        );
        assert!(
            response(serde_json::json!({ "index": 0, "embedding": [0.1] }))
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn embedding_response_rejects_reserved_keys_in_rest() {
        let mut resp = EmbeddingResponse {
            model: "m".to_owned(),
            data: vec![],
            rest: Map::new(),
        };
        resp.rest.insert("data".to_owned(), serde_json::json!([]));
        assert!(resp.validate().is_err());
    }

    fn rerank_request(model: &str) -> RerankRequest {
        RerankRequest {
            model: model.to_owned(),
            query: "what is rust".to_owned(),
            documents: vec!["a systems language".to_owned(), "a card game".to_owned()],
            top_n: None,
            rest: Map::new(),
        }
    }

    #[test]
    fn rerank_request_round_trips_with_top_n() {
        let json = serde_json::json!({
            "model": "m",
            "query": "what is rust",
            "documents": ["a systems language", "a card game"],
            "top_n": 1,
            "truncate": true,
        });
        let req: RerankRequest = serde_json::from_value(json).expect("parse request");
        assert_eq!(req.top_n, Some(1));
        // Unnamed fields land in `rest`, not on named fields.
        assert!(req.rest.contains_key("truncate"));
        assert!(!req.rest.contains_key("model"));
        assert!(!req.rest.contains_key("query"));
        assert!(!req.rest.contains_key("documents"));
        assert!(!req.rest.contains_key("top_n"));
        let reparsed: RerankRequest =
            serde_json::from_value(serde_json::to_value(&req).expect("serialize"))
                .expect("reparse");
        assert_eq!(req, reparsed);
    }

    #[test]
    fn rerank_request_omits_an_absent_top_n() {
        let req = rerank_request("m");
        assert_eq!(req.top_n, None);
        // An absent top_n neither errors nor serializes as null.
        assert!(
            !serde_json::to_value(&req)
                .expect("serialize")
                .as_object()
                .expect("object")
                .contains_key("top_n")
        );
        let reparsed: RerankRequest =
            serde_json::from_value(serde_json::to_value(&req).expect("serialize"))
                .expect("reparse");
        assert_eq!(req, reparsed);
    }

    #[test]
    fn rerank_request_rejects_empty_model_query_and_documents() {
        assert!(rerank_request("  ").validate().is_err());
        let empty_query = RerankRequest {
            query: "  ".to_owned(),
            ..rerank_request("m")
        };
        assert!(empty_query.validate().is_err());
        let no_documents = RerankRequest {
            documents: vec![],
            ..rerank_request("m")
        };
        assert!(no_documents.validate().is_err());
        assert!(rerank_request("m").validate().is_ok());
    }

    #[test]
    fn rerank_request_rejects_reserved_keys_in_rest() {
        let mut req = rerank_request("m");
        req.rest
            .insert("documents".to_owned(), serde_json::json!(["x"]));
        assert!(req.validate().is_err());
    }

    #[test]
    fn rerank_response_round_trips_and_preserves_usage() {
        let json = serde_json::json!({
            "model": "backend",
            "results": [
                { "index": 0, "relevance_score": 0.9, "document": { "text": "a systems language" } },
                { "index": 1, "relevance_score": 0.1 }
            ],
            "usage": { "total_tokens": 12 },
        });
        let resp: RerankResponse = serde_json::from_value(json).expect("parse response");
        assert!(resp.validate().is_ok());
        assert!(resp.rest.contains_key("usage"));
        let reparsed: RerankResponse =
            serde_json::from_value(serde_json::to_value(&resp).expect("serialize"))
                .expect("reparse");
        assert_eq!(resp, reparsed);
    }

    #[test]
    fn rerank_response_rejects_malformed_results() {
        // WIRE-002: a structurally broken result is an upstream-protocol failure.
        let response = |result: Value| RerankResponse {
            model: "m".to_owned(),
            results: vec![result],
            rest: Map::new(),
        };
        assert!(response(serde_json::json!(42)).validate().is_err());
        assert!(
            response(serde_json::json!({ "index": 0 }))
                .validate()
                .is_err()
        );
        assert!(
            response(serde_json::json!({ "relevance_score": 0.9 }))
                .validate()
                .is_err()
        );
        assert!(
            response(serde_json::json!({ "index": 0, "relevance_score": 0.9 }))
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn rerank_response_rejects_reserved_keys_in_rest() {
        let mut resp = RerankResponse {
            model: "m".to_owned(),
            results: vec![],
            rest: Map::new(),
        };
        resp.rest
            .insert("results".to_owned(), serde_json::json!([]));
        assert!(resp.validate().is_err());
    }

    #[test]
    fn model_info_serializes_kind_in_catalog_spelling() {
        let info = |kind: ModelKind| ModelInfo {
            id: "m".to_owned(),
            object: "model",
            kind,
            description: "d".to_owned(),
            context: 8192,
            thinking: ThinkingMode::Never,
            capabilities: Capabilities::default(),
        };
        for (kind, spelling) in [
            (ModelKind::Chat, "chat"),
            (ModelKind::Embedding, "embedding"),
            (ModelKind::Classifier, "classifier"),
        ] {
            let json = serde_json::to_value(info(kind)).expect("serialize");
            assert_eq!(json.get("kind").and_then(Value::as_str), Some(spelling));
        }
    }
}
