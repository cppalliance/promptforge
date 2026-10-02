//! Wire types for the chat-completions protocol: messages, tool schemas,
//! tool calls, and completion results. The constructors a Harness that
//! ran no transport builds a completion from sit in the `canned` sibling.

#[path = "wire-canned.rs"]
mod canned;

use promptforge_types::metrics::CallMetrics;
use serde_json::Value;

/// A single chat message.
///
/// A plain `user` message serializes to just `{"role":..,"content":..}`; the
/// optional `tool_call_id` and `tool_calls` fields are emitted only when set,
/// which keeps the wire shape of ordinary messages unchanged.
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
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge::model::Message;
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

    /// Constructs a plain `assistant` text turn (no `tool_calls` field).
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

    /// Returns the message text, or `""` when the content is a
    /// content-parts array rather than a string (only the Engine builds
    /// that form).
    #[must_use]
    pub fn content(&self) -> &str {
        self.content.as_str().unwrap_or("")
    }
}

/// A tool advertised to the model, in the `OpenAI` function-calling shape.
///
/// When serialized into a request the wrapping code turns this into
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
    /// Returns the tool's wire name: never empty, and only `[A-Za-z0-9_.-]`.
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

/// A tool invocation requested by the model.
///
/// `OpenAI` returns tool calls with `function.arguments` as a JSON-encoded
/// string; the wire decoder stores that string decoded into a JSON object,
/// and fails the turn when the arguments are missing, not a string, not
/// valid JSON, or not an object.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolCall {
    /// The id the model assigned to this call, echoed back with its result.
    pub(crate) id: String,
    /// The name of the tool to invoke.
    pub(crate) name: String,
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

    /// Returns a typed, borrowed view of the call's arguments.
    ///
    /// The raw wire JSON - a [`serde_json::Value`] - stays crate-private;
    /// callers inspect the arguments through [`ToolArguments`] (canonical
    /// JSON text, key presence, argument names).
    #[must_use]
    pub fn arguments(&self) -> ToolArguments<'_> {
        ToolArguments {
            value: &self.arguments,
        }
    }
}

/// A typed, borrowed view over one [`ToolCall`]'s arguments.
///
/// The arguments are always a JSON object: the wire decoder and
/// [`ToolCall::from_parts`] both refuse any other value. This view exposes
/// them without leaking a [`serde_json::Value`] into the public API. The
/// raw `Value` is confined to crate-private wire code.
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

    /// Returns whether the arguments object has no keys.
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

/// The outcome of a completion round trip.
///
/// `Eq` holds because [`ToolCall`] arguments are a [`serde_json::Value`],
/// which implements `Eq`, so structural equivalence over the outcome is
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
/// # async fn example(completion: promptforge::model::Completion) {
/// use promptforge::model::CompletionResult;
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

/// What a transport sent and what it read for one round, as opaque JSON.
///
/// A broker that speaks a JSON wire format can attach the pair to the
/// [`Completion`] it returns, so a Host's debug capture shows exactly what
/// crossed the wire. The Engine never looks inside either value: it hands
/// them to the debug capture and nothing else. Build one with
/// [`RawExchange::new`]; a completion built without a transport carries
/// none.
///
/// # Examples
///
/// ```
/// use promptforge::model::RawExchange;
/// use serde_json::json;
///
/// let raw = RawExchange::new(
///     json!({ "model": "m", "messages": [] }),
///     json!({ "choices": [] }),
/// );
/// assert_eq!(raw.request()["model"], "m");
/// assert_eq!(raw.response()["choices"], json!([]));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RawExchange {
    /// The request as it left for the backend.
    pub(crate) request: Value,
    /// The response as the backend returned it, reassembled into the
    /// buffered chat-completion shape when it streamed.
    pub(crate) response: Value,
}

impl RawExchange {
    /// A raw exchange from the request a transport sent and the response it
    /// read. Neither value is checked.
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

/// A parsed chat-completions round trip, including metadata later steps need.
///
/// [`CompletionResult`] remains the decision the tool loop matches on.
/// `finish_reason` and `reasoning_content` sit beside it so observers can
/// report payload-free signals without reading the raw bodies, and the call
/// metadata - the serving model plus everything the call measured, as
/// [`CallMetrics`] - is included for attribution and accounting. A broker
/// may also attach the round's [`RawExchange`] for debug capture. Outside
/// crates read through the accessor methods.
#[derive(Debug)]
#[non_exhaustive]
pub struct Completion {
    /// The text or tool-call outcome the tool loop consumes.
    pub(crate) result: CompletionResult,
    /// The choice's `finish_reason`, when the backend supplied one.
    pub(crate) finish_reason: Option<String>,
    /// The message's reasoning side channel, when the backend supplied one.
    pub(crate) reasoning_content: Option<String>,
    /// The model that served the call, empty when the body named none.
    pub(crate) model: String,
    /// Everything the call measured, when anything reported: the backend's
    /// `usage` and timing extensions, and the client's own clock.
    pub(crate) metrics: Option<CallMetrics>,
    /// One line per response metadata section that was present but
    /// malformed and degraded to `None`; empty for a well-formed body. The
    /// Engine reports each line as a `model_metadata_degraded` event.
    pub(crate) metadata_diagnostics: Vec<String>,
    /// The request and response the broker attached for debug capture.
    pub(crate) raw: Option<RawExchange>,
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

    /// Returns one line per response metadata section that was present but
    /// malformed and degraded to `None`; empty for a well-formed body. The
    /// Engine reports each line as a `model_metadata_degraded` event.
    #[must_use]
    pub fn metadata_diagnostics(&self) -> &[String] {
        &self.metadata_diagnostics
    }

    /// Returns the model that served the call, as the backend named it in the
    /// response body (empty when the body named none).
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns everything the call measured, when anything reported: token
    /// accounting, llama.cpp's `timings`, vLLM's `metrics`, and the timing
    /// the client measured on its own clock. Each section is absent when
    /// its source did not report it.
    #[must_use]
    pub fn metrics(&self) -> Option<&CallMetrics> {
        self.metrics.as_ref()
    }

    /// Returns the request and response the broker attached for debug
    /// capture, when it attached them.
    #[must_use]
    pub fn raw(&self) -> Option<&RawExchange> {
        self.raw.as_ref()
    }
}
