//! Wire types for the chat-completions protocol: messages, tool schemas,
//! tool calls, and completion results.

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
    /// The message text.
    pub(crate) content: String,
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
    /// use promptforge_core::client::Message;
    ///
    /// let message = Message::user("hello");
    /// assert_eq!(message.role(), "user");
    /// assert_eq!(message.content(), "hello");
    /// ```
    #[must_use]
    pub fn user(content: impl Into<String>) -> Message {
        Message {
            role: "user".into(),
            content: content.into(),
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
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: None,
        }
    }

    /// Construct a plain `assistant` text turn (no `tool_calls` field).
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Message {
        Message {
            role: "assistant".into(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Construct the `assistant` turn that requested tool calls.
    ///
    /// `raw_tool_calls` is the backend's `tool_calls` array echoed back
    /// verbatim so the conversation history matches what the model emitted.
    #[must_use]
    pub(crate) fn assistant_tool_calls(raw_tool_calls: Vec<Value>) -> Message {
        Message {
            role: "assistant".into(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: Some(raw_tool_calls),
        }
    }

    /// Returns the message role (`system`, `user`, `assistant`, or `tool`).
    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    /// Returns the message text.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
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
    pub(crate) name: String,
    /// A one-sentence description shown to the model.
    pub(crate) description: String,
    /// The JSON Schema for the tool's parameters.
    pub(crate) parameters: Value,
}

/// The reason a [`ToolSchema`] could not be built from its wire parts.
///
/// Crate-private: `ToolSchema` is built only inside the crate (from the
/// [`crate::tools::Tool`] contract), so the raw-`Value` validation and its
/// error stay internal and never surface in the public API (client F8,
/// lib F3).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum ToolSchemaError {
    /// The wire name was empty or held a character outside `[A-Za-z0-9_.-]`.
    #[error("invalid tool wire name {name:?}: {reason}")]
    #[non_exhaustive]
    InvalidName {
        /// The rejected wire name.
        name: String,
        /// Why it was rejected.
        reason: &'static str,
    },
    /// The parameters JSON Schema was not a JSON object.
    #[error("tool {name:?} parameters schema must be a JSON object")]
    #[non_exhaustive]
    NonObjectSchema {
        /// The tool whose schema was rejected.
        name: String,
    },
}

impl ToolSchema {
    /// Builds a tool schema, validating the wire name and object-shaped schema.
    ///
    /// Crate-private (client F8, lib F3): the raw [`serde_json::Value`] schema
    /// enters here only from the internal [`crate::tools::Tool::parameters_schema`]
    /// contract, so the raw JSON never appears in a public constructor
    /// signature. External callers advertise tools through the `Tool` trait and
    /// the executor, not by hand-building a `ToolSchema`.
    ///
    /// # Errors
    /// Returns [`ToolSchemaError::InvalidName`] when `name` is empty or contains
    /// a character outside `[A-Za-z0-9_.-]`, and
    /// [`ToolSchemaError::NonObjectSchema`] when `parameters` is not a JSON
    /// object, so a tool can never be advertised to the model with an unusable
    /// name or a non-object JSON Schema (F7).
    pub(crate) fn new(
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
    pub(crate) id: String,
    /// The name of the tool to invoke.
    pub(crate) name: String,
    /// The parsed arguments for the call.
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
/// `Value` is confined to crate-private dialect/wire code.
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
/// # async fn example(completion: promptforge_core::client::Completion) {
/// use promptforge_core::client::CompletionResult;
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
/// report payload-free signals without reading the raw bodies. The request and
/// response bodies are `pub(crate)` for the opt-in [`crate::debug::DebugCapture`]
/// seam; they are not part of the public host API.
#[derive(Debug)]
#[non_exhaustive]
pub struct Completion {
    /// The text or tool-call outcome the tool loop consumes.
    pub(crate) result: CompletionResult,
    /// The choice's `finish_reason`, when the backend supplied one.
    pub(crate) finish_reason: Option<String>,
    /// The message's reasoning side channel, when the backend supplied one.
    pub(crate) reasoning_content: Option<String>,
    /// The JSON body sent to the gateway.
    pub(crate) request_body: Value,
    /// The JSON body returned by the gateway.
    pub(crate) response_body: Value,
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
}
