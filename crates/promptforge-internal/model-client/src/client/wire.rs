//! Wire types for the chat-completions protocol: messages, tool schemas,
//! tool calls, and completion results. The constructors a Harness that
//! ran no transport builds a completion from sit in the `canned` sibling.

#[path = "wire-canned.rs"]
mod canned;

use promptforge_types::metrics::CallMetrics;
use serde_json::Value;

/// One message in a chat-completions conversation.
///
/// A message serializes to a JSON object with `role` and `content` keys.
/// The optional `tool_call_id` and `tool_calls` keys appear only when set,
/// so a plain `user` message serializes to just `{"role":..,"content":..}`.
// `PartialEq`/`Eq` compare messages structurally. `serde_json::Value`
// implements `Eq` (its `Number` compares/hashes float bits), so the
// `tool_calls` field does not block a total equivalence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct Message {
    /// The role: `system`, `user`, `assistant`, or `tool`.
    pub(crate) role: String,
    /// The message content, serialized into the request verbatim: a JSON
    /// string for a plain text message (every inherent constructor), or an
    /// OpenAI content-parts array for a multimodal message built through
    /// [`crate::detail::message_from_validated_parts`].
    pub(crate) content: Value,
    /// For a `tool` message, the id of the tool call this result answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_call_id: Option<String>,
    /// For an `assistant` turn that requested tools, the raw `tool_calls` array
    /// as received from the backend, echoed back verbatim on the live path. The
    /// projection path instead re-renders each call from its neutral
    /// `ToolCallRecord` into the OpenAI function-call shape, so key order and
    /// whitespace can differ from the provider's original.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_calls: Option<Vec<Value>>,
}

impl Message {
    /// Constructs a `user` message.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Message {
        Message {
            role: "user".into(),
            content: Value::String(content.into()),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Constructs a `tool` message holding the result of a tool call.
    ///
    /// `tool_call_id` must match the `id` of the [`ToolCall`] this answers.
    #[must_use]
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Message {
        Message {
            role: "tool".into(),
            content: Value::String(content.into()),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: None,
        }
    }

    /// Constructs a plain `assistant` text turn that serializes to just
    /// `role` and `content`.
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Message {
        Message {
            role: "assistant".into(),
            content: Value::String(content.into()),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Returns the message role (`system`, `user`, `assistant`, or `tool`).
    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    /// Returns the message text, or `""` when the content is a multimodal
    /// content-parts array.
    ///
    /// Only the Engine builds multimodal messages. The `user`, `tool`, and
    /// `assistant` constructors always produce text.
    #[must_use]
    pub fn content(&self) -> &str {
        self.content.as_str().unwrap_or("")
    }
}

/// A tool offered to the model: a name, a description, and a JSON Schema
/// for the tool's parameters.
///
/// A request lists each schema as an `OpenAI` function tool:
/// `{"type":"function","function":{"name":..,"description":..,"parameters":..}}`.
// `PartialEq`/`Eq` compare schemas structurally. `serde_json::Value`
// implements `Eq`, so the `parameters` schema does not block equivalence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct ToolSchema {
    /// The tool's wire name; the Engine reads it through
    /// [`crate::detail::tool_schema_name`].
    pub(crate) name: String,
    /// A one-sentence description shown to the model; the Engine reads it
    /// through [`crate::detail::tool_schema_description`].
    pub(crate) description: String,
    /// The JSON Schema for the tool's parameters.
    pub(crate) parameters: Value,
}

impl ToolSchema {
    /// Returns the tool's name as sent to the model. It holds one or more
    /// characters, all from `[A-Za-z0-9_.-]`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the one-sentence description shown to the model.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the JSON Schema for the tool's parameters, always a JSON
    /// object.
    #[must_use]
    pub fn parameters(&self) -> &Value {
        &self.parameters
    }
}

/// The reason a [`ToolSchema`] could not be built from its wire parts.
///
/// `ToolSchema` is built only inside the Engine (from the executor's `Tool`
/// contract, through [`crate::detail::tool_schema_new`]), so the raw-`Value`
/// validation and its error stay off the facade. The type is public so
/// `promptforge-engine` can box it as an error source.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ToolSchemaError {
    /// The wire name was empty or held a character outside `[A-Za-z0-9_.-]`.
    #[error("invalid tool wire name {name:?}: {reason}")]
    InvalidName {
        /// The rejected wire name.
        name: String,
        /// Why it was rejected.
        reason: &'static str,
    },
    /// The parameters JSON Schema was not a JSON object.
    #[error("tool {name:?} parameters schema must be a JSON object")]
    NonObjectSchema {
        /// The tool whose schema was rejected.
        name: String,
    },
}

/// A tool call the model asked for: a call id, a tool name, and arguments.
///
/// `OpenAI` sends a call's `function.arguments` as a JSON-encoded string.
/// A decoded `ToolCall` holds that string parsed into a JSON object.
/// Decoding fails with an error when the arguments are missing, are not a
/// string, are not valid JSON, or do not parse to an object.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolCall {
    /// The id the model assigned to this call, echoed back with its result.
    pub(super) id: String,
    /// The name of the tool to invoke.
    pub(super) name: String,
    /// The parsed arguments for the call. The raw wire JSON stays
    /// crate-private: the Harness inspects arguments through
    /// [`ToolCall::arguments`].
    pub(crate) arguments: Value,
}

impl ToolCall {
    /// Returns the id the model assigned to this call.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the name of the tool to invoke.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns a borrowed view of the call's arguments.
    ///
    /// The [`ToolArguments`] view gives the arguments as canonical JSON text,
    /// says whether a key is present, and lists the argument names.
    #[must_use]
    pub fn arguments(&self) -> ToolArguments<'_> {
        ToolArguments {
            value: &self.arguments,
        }
    }
}

/// A borrowed, read-only view of one [`ToolCall`]'s arguments.
///
/// The arguments are always a JSON object: decoding and
/// [`ToolCall::from_parts`] both reject any other value.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ToolArguments<'a> {
    value: &'a Value,
}

impl ToolArguments<'_> {
    /// Returns the arguments object serialized as canonical JSON text.
    #[must_use]
    pub fn to_json_string(&self) -> String {
        self.value.to_string()
    }

    /// Returns whether the arguments object is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self.value {
            Value::Null => true,
            Value::Object(map) => map.is_empty(),
            _ => false,
        }
    }

    /// Returns whether the arguments object has a top-level key named `key`.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.value
            .as_object()
            .is_some_and(|map| map.contains_key(key))
    }

    /// Returns the top-level argument names of the arguments object.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.value
            .as_object()
            .into_iter()
            .flat_map(|map| map.keys().map(String::as_str))
    }
}

/// What the model returned for one completion: a final text reply or a
/// request to call tools.
///
/// For a tool request, read each call through [`ToolCall::id`],
/// [`ToolCall::name`], and [`ToolCall::arguments`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompletionResult {
    /// The model returned a final text reply.
    Text(String),
    /// The model asked to call one or more tools.
    ToolCalls(Vec<ToolCall>),
}

/// The raw JSON request and response of one model call, kept for debug
/// capture.
///
/// The caller that sends a request in a JSON wire format can attach the
/// pair to the [`Completion`] it returns, so the application's debug
/// capture shows exactly what was sent and received. The Engine treats
/// both values as opaque and passes them only to the debug capture.
/// Build one with [`RawExchange::new`]. `Completion::from_result` builds a
/// completion with `raw` set to `None`, and the caller attaches one with
/// `Completion::with_raw`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RawExchange {
    /// The request as it left for the backend.
    request: Value,
    /// The response as the backend returned it, reassembled into the
    /// buffered chat-completion shape when it streamed.
    response: Value,
}

impl RawExchange {
    /// Creates a raw exchange from the request that was sent and the
    /// response that was read. It accepts any two JSON values and stores
    /// them as given.
    #[must_use]
    pub fn new(request: Value, response: Value) -> RawExchange {
        RawExchange { request, response }
    }

    /// Returns the request as it left for the backend.
    #[must_use]
    pub fn request(&self) -> &Value {
        &self.request
    }

    /// Returns the response as the backend returned it.
    #[must_use]
    pub fn response(&self) -> &Value {
        &self.response
    }
}

/// The parsed result of one chat-completions call, with its metadata.
///
/// The [`CompletionResult`] is what the tool loop matches on. Beside it, a
/// completion carries the backend's `finish_reason` and reasoning text, so
/// observers can report them straight from the completion. It also
/// carries the name of the model that served the call and the
/// [`CallMetrics`] the call measured, for attribution and accounting. The
/// caller may also attach the call's [`RawExchange`] for debug capture.
#[derive(Debug)]
#[non_exhaustive]
pub struct Completion {
    /// The text or tool-call outcome the tool loop consumes.
    pub(crate) result: CompletionResult,
    /// The choice's `finish_reason`, when the backend supplied one.
    finish_reason: Option<String>,
    /// The message's reasoning side channel, when the backend supplied one.
    reasoning_content: Option<String>,
    /// The model that served the call, empty when the body named none.
    model: String,
    /// Everything the call measured, when anything reported: the backend's
    /// `usage` and timing extensions, and the client's own clock.
    metrics: Option<CallMetrics>,
    /// One line per response metadata section that was present but
    /// malformed and degraded to `None`; empty for a well-formed body. The
    /// Engine reports each line as a `model_metadata_degraded` event.
    pub(crate) metadata_diagnostics: Vec<String>,
    /// The request and response the broker attached for debug capture.
    raw: Option<RawExchange>,
}

impl Completion {
    /// Returns the text or tool-call outcome the tool loop consumes.
    #[must_use]
    pub fn result(&self) -> &CompletionResult {
        &self.result
    }

    /// Returns the `finish_reason` the backend gave for the reply, when it
    /// supplied one.
    #[must_use]
    pub fn finish_reason(&self) -> Option<&str> {
        self.finish_reason.as_deref()
    }

    /// Returns the reasoning text the backend sent beside the reply, when it
    /// sent any. It stays separate from the answer.
    #[must_use]
    pub fn reasoning_content(&self) -> Option<&str> {
        self.reasoning_content.as_deref()
    }

    /// Returns one line for each metadata section of the response that was
    /// present but malformed and so was treated as absent. The list is
    /// empty for a well-formed response. The Engine reports each line as a
    /// `model_metadata_degraded` event.
    #[must_use]
    pub fn metadata_diagnostics(&self) -> &[String] {
        &self.metadata_diagnostics
    }

    /// Returns the name of the model that served the call.
    ///
    /// That is the name last set through
    /// [`with_model`](Completion::with_model), or else the name the
    /// completion was built with. A completion decoded from a response is
    /// built with the model the response body named, or an empty name when
    /// the body omits one.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns what the call measured, or `None` when nothing reported. It
    /// covers token usage, llama.cpp's `timings`, vLLM's `metrics`, and the
    /// timing the client measured on its own clock. Each section is present
    /// only when its source reported it.
    #[must_use]
    pub fn metrics(&self) -> Option<&CallMetrics> {
        self.metrics.as_ref()
    }

    /// Returns the raw request and response the caller attached for debug
    /// capture, if it attached them.
    #[must_use]
    pub fn raw(&self) -> Option<&RawExchange> {
        self.raw.as_ref()
    }
}
