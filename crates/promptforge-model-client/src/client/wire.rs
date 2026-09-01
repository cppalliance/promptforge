//! Wire types for the chat-completions protocol: messages, tool schemas,
//! tool calls, and completion results.

use promptforge_core_support::events::{ClientTiming, LlamaTimings, Usage, VllmMetrics};
use serde_json::Value;

/// A single chat message.
///
/// A plain `user` message serializes to just `{"role":..,"content":..}`; the
/// optional `tool_call_id` and `tool_calls` fields are emitted only when set,
/// which keeps the wire shape of ordinary messages unchanged.
// `PartialEq`/`Eq` compare messages structurally (F9). `serde_json::Value`
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
    /// [`Message::from_validated_parts`].
    pub(crate) content: Value,
    /// For a `tool` message, the id of the tool call this result answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_call_id: Option<String>,
    /// For an `assistant` turn that requested tools, the raw `tool_calls` array
    /// as received from the backend, echoed back verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_calls: Option<Vec<Value>>,
}

impl Message {
    /// Construct a `user` message.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_model_client::client::Message;
    ///
    /// let message = Message::user("hello");
    /// assert_eq!(message.role(), "user");
    /// assert_eq!(message.content(), "hello");
    /// ```
    #[must_use]
    pub fn user(content: impl Into<String>) -> Message {
        Message {
            role: "user".into(),
            content: Value::String(content.into()),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Construct a `tool` message carrying the result of a tool call.
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

    /// Construct a plain `assistant` text turn (no `tool_calls` field).
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Message {
        Message {
            role: "assistant".into(),
            content: Value::String(content.into()),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Constructs a message from parts a caller has already validated.
    ///
    /// `role` is one of the wire roles (`system`, `user`, `assistant`,
    /// `tool`). `content` is the raw wire content value - a string for a
    /// plain message or an OpenAI content-parts array for a multimodal one -
    /// and serializes into the request verbatim.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for the agent executor, whose
    /// protocol layer validates author-built message tables once and hands
    /// the validated parts here; not host API.
    #[doc(hidden)]
    #[must_use]
    pub fn from_validated_parts(
        role: impl Into<String>,
        content: Value,
        tool_call_id: Option<String>,
        tool_calls: Option<Vec<Value>>,
    ) -> Message {
        Message {
            role: role.into(),
            content,
            tool_call_id,
            tool_calls,
        }
    }

    /// Construct the `assistant` turn that requested tool calls.
    ///
    /// `raw_tool_calls` is the backend's `tool_calls` array echoed back
    /// verbatim so the conversation history matches what the model emitted.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for the executor's tool loop, not
    /// host API.
    #[doc(hidden)]
    #[must_use]
    pub fn assistant_tool_calls(raw_tool_calls: Vec<Value>) -> Message {
        Message {
            role: "assistant".into(),
            content: Value::String(String::new()),
            tool_call_id: None,
            tool_calls: Some(raw_tool_calls),
        }
    }

    /// Returns the message role (`system`, `user`, `assistant`, or `tool`).
    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    /// Returns the message text, or `""` when the content is a
    /// content-parts array rather than a string (only
    /// [`Message::from_validated_parts`] builds that form).
    #[must_use]
    pub fn content(&self) -> &str {
        self.content.as_str().unwrap_or("")
    }
}

/// A tool advertised to the model, in the `OpenAI` function-calling shape.
///
/// When serialized into a request the wrapping code turns this into
/// `{"type":"function","function":{"name":..,"description":..,"parameters":..}}`.
// `PartialEq`/`Eq` compare schemas structurally (F9). `serde_json::Value`
// implements `Eq`, so the `parameters` schema does not block equivalence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct ToolSchema {
    /// The tool's wire name.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for the executor's dispatch map,
    /// not host API.
    #[doc(hidden)]
    pub name: String,
    /// A one-sentence description shown to the model.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for the executor's scope tests,
    /// not host API.
    #[doc(hidden)]
    pub description: String,
    /// The JSON Schema for the tool's parameters.
    pub(crate) parameters: Value,
}

/// The reason a [`ToolSchema`] could not be built from its wire parts.
///
/// `#[doc(hidden)]`: `ToolSchema` is built only inside the workspace (from the
/// executor's `Tool` contract), so the raw-`Value` validation and its error
/// stay out of the documented API (client F8, lib F3). The type is visible
/// only so the companion `promptforge-core` crate can box it as an error
/// source.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[doc(hidden)]
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

impl ToolSchema {
    /// Builds a tool schema, validating the wire name and object-shaped schema.
    ///
    /// `#[doc(hidden)]` (client F8, lib F3): the raw [`serde_json::Value`]
    /// schema enters here only from the executor's internal tool contract, so
    /// the raw JSON never appears in a documented constructor signature.
    /// External callers advertise tools through the `Tool` trait and the
    /// executor, not by hand-building a `ToolSchema`.
    ///
    /// # Errors
    /// Returns [`ToolSchemaError::InvalidName`] when `name` is empty or contains
    /// a character outside `[A-Za-z0-9_.-]`, and
    /// [`ToolSchemaError::NonObjectSchema`] when `parameters` is not a JSON
    /// object, so a tool can never be advertised to the model with an unusable
    /// name or a non-object JSON Schema (F7).
    #[doc(hidden)]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> std::result::Result<ToolSchema, ToolSchemaError> {
        let name = name.into();
        if name.is_empty() {
            return Err(ToolSchemaError::InvalidName {
                name,
                reason: "must not be empty",
            });
        }
        if !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        {
            return Err(ToolSchemaError::InvalidName {
                name,
                reason: "may contain only [A-Za-z0-9_.-]",
            });
        }
        if !parameters.is_object() {
            return Err(ToolSchemaError::NonObjectSchema { name });
        }
        Ok(ToolSchema {
            name,
            description: description.into(),
            parameters,
        })
    }
}

/// A tool invocation requested by the model.
///
/// `OpenAI` returns tool calls with `function.arguments` as a JSON-encoded
/// string; this type holds that string parsed into a [`Value`] (falling back to
/// a string `Value` if it is not valid JSON).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolCall {
    /// The id the model assigned to this call, echoed back with its result.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for the executor's tool loop; read
    /// through [`ToolCall::id`] in host code.
    #[doc(hidden)]
    pub id: String,
    /// The name of the tool to invoke.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for the executor's tool loop; read
    /// through [`ToolCall::name`] in host code.
    #[doc(hidden)]
    pub name: String,
    /// The parsed arguments for the call.
    ///
    /// `#[doc(hidden)]` (F8): the raw wire JSON stays out of the documented
    /// API; host code inspects arguments through [`ToolCall::arguments`].
    #[doc(hidden)]
    pub arguments: Value,
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

    /// Returns a typed, borrowed view of the call's arguments.
    ///
    /// F8: the public API no longer hands out a raw [`serde_json::Value`]. The
    /// raw wire JSON stays crate-private; callers inspect the arguments through
    /// [`ToolArguments`] (canonical JSON text, key presence, argument names).
    #[must_use]
    pub fn arguments(&self) -> ToolArguments<'_> {
        ToolArguments {
            value: &self.arguments,
        }
    }
}

/// A typed, borrowed view over one [`ToolCall`]'s arguments.
///
/// Tool-call arguments arrive as arbitrary wire JSON; this view exposes them
/// without leaking a [`serde_json::Value`] into the public API (F8). The raw
/// `Value` is confined to crate-private wire code.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ToolArguments<'a> {
    value: &'a Value,
}

impl ToolArguments<'_> {
    /// Returns the arguments serialized as canonical JSON text.
    #[must_use]
    pub fn to_json_string(&self) -> String {
        self.value.to_string()
    }

    /// Returns whether the call carried no arguments (a `null` payload or an
    /// empty JSON object).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self.value {
            Value::Null => true,
            Value::Object(map) => map.is_empty(),
            _ => false,
        }
    }

    /// Returns whether a top-level argument named `key` is present, when the
    /// arguments are a JSON object.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.value
            .as_object()
            .is_some_and(|map| map.contains_key(key))
    }

    /// Returns the top-level argument names, when the arguments are a JSON
    /// object (an empty iterator otherwise).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.value
            .as_object()
            .into_iter()
            .flat_map(|map| map.keys().map(String::as_str))
    }
}

/// One live increment from a streaming completion.
///
/// [`GatewayClient::complete`](crate::client::GatewayClient::complete) invokes
/// its delta callback with these as the stream arrives: answer text and the
/// reasoning side channel stay separated so a consumer can render them
/// differently. Tool-call fragments are never surfaced as deltas; they buffer
/// inside the client until the batch is complete and validated.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamDelta {
    /// A fragment of the assistant's answer text.
    Text(String),
    /// A fragment of the reasoning side channel, never part of the answer.
    Reasoning(String),
}

/// The outcome of a completion round trip.
///
/// `Eq` holds because [`ToolCall`] arguments are a [`serde_json::Value`],
/// which implements `Eq` (F9), so structural equivalence over the outcome is
/// total.
///
/// # Examples
///
/// A caller matches the outcome and, for a tool turn, reads each call's typed
/// accessors ([`ToolCall::id`], [`ToolCall::name`], [`ToolCall::arguments`]) and
/// the borrowed [`ToolArguments`] view. Obtaining a result performs gateway
/// I/O, so the example is `no_run`:
///
/// ```no_run
/// # async fn example(completion: promptforge_model_client::client::Completion) {
/// use promptforge_model_client::client::CompletionResult;
///
/// match completion.result() {
///     CompletionResult::Text(reply) => println!("text: {reply}"),
///     CompletionResult::ToolCalls(calls) => {
///         for call in calls {
///             let args = call.arguments();
///             println!("{} -> {} {}", call.id(), call.name(), args.to_json_string());
///             let _ = args.contains("query");
///         }
///     }
///     _ => {}
/// }
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompletionResult {
    /// The model returned a final text reply.
    Text(String),
    /// The model asked to call one or more tools.
    ToolCalls(Vec<ToolCall>),
}

/// A parsed chat-completions round trip, including metadata later steps need.
///
/// [`CompletionResult`] remains the decision the tool loop matches on.
/// `finish_reason` and `reasoning_content` ride beside it so observers can
/// report payload-free signals without reading the raw bodies, and the call
/// metadata - the serving model plus the canonical metrics vocabulary
/// re-exported at the crate root ([`Usage`], [`LlamaTimings`],
/// [`VllmMetrics`], [`ClientTiming`]) - rides along for attribution and
/// accounting. The fields are `#[doc(hidden)]` cross-crate seams for the
/// executor's tool loop and the opt-in debug-capture seam; they are not part
/// of the public host API, which reads through the accessor methods.
#[derive(Debug)]
#[non_exhaustive]
pub struct Completion {
    /// The text or tool-call outcome the tool loop consumes.
    #[doc(hidden)]
    pub result: CompletionResult,
    /// The choice's `finish_reason`, when the backend supplied one.
    #[doc(hidden)]
    pub finish_reason: Option<String>,
    /// The message's reasoning side channel, when the backend supplied one.
    #[doc(hidden)]
    pub reasoning_content: Option<String>,
    /// The model that served the call, empty when the body named none.
    #[doc(hidden)]
    pub model: String,
    /// Token accounting, when the backend reported `usage`.
    #[doc(hidden)]
    pub usage: Option<Usage>,
    /// llama.cpp's `timings` extension, when that backend served the call.
    #[doc(hidden)]
    pub llama_timings: Option<LlamaTimings>,
    /// vLLM's `metrics` extension, when that backend served the call.
    #[doc(hidden)]
    pub vllm_metrics: Option<VllmMetrics>,
    /// Timing measured by this client's own clock: time to first token,
    /// mean inter-token latency, and end-to-end wall time for the stream.
    #[doc(hidden)]
    pub client_timing: Option<ClientTiming>,
    /// The JSON body sent to the gateway.
    #[doc(hidden)]
    pub request_body: Value,
    /// The buffered chat-completion body reassembled from the streamed
    /// chunks, in the same shape a non-streaming backend would return.
    #[doc(hidden)]
    pub response_body: Value,
}

impl Completion {
    /// Returns the text or tool-call outcome the tool loop consumes.
    #[must_use]
    pub fn result(&self) -> &CompletionResult {
        &self.result
    }

    /// Returns the choice's `finish_reason`, when the backend supplied one.
    #[must_use]
    pub fn finish_reason(&self) -> Option<&str> {
        self.finish_reason.as_deref()
    }

    /// Returns the reasoning side channel, when the backend supplied one. It is
    /// never promoted into the answer.
    #[must_use]
    pub fn reasoning_content(&self) -> Option<&str> {
        self.reasoning_content.as_deref()
    }

    /// Returns the model that served the call, as the backend named it in the
    /// response body (empty when the body named none).
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the backend's token accounting, when it reported `usage`.
    #[must_use]
    pub fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    /// Returns llama.cpp's `timings` for the call, when that backend served
    /// it.
    #[must_use]
    pub fn llama_timings(&self) -> Option<&LlamaTimings> {
        self.llama_timings.as_ref()
    }

    /// Returns vLLM's per-request `metrics`, when that backend served the
    /// call.
    #[must_use]
    pub fn vllm_metrics(&self) -> Option<&VllmMetrics> {
        self.vllm_metrics.as_ref()
    }

    /// Returns the timing this client measured on its own clock, when the
    /// transport measured one.
    #[must_use]
    pub fn client_timing(&self) -> Option<&ClientTiming> {
        self.client_timing.as_ref()
    }
}
