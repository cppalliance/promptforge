//! An `OpenAI`-compatible chat completions client, pointed at the gateway.
//!
//! The client speaks the non-streaming `/chat/completions` shape: a list of
//! messages in, and either one text reply out or the tool calls the model
//! asked for. [`GatewayClient::complete`] sends a `tools` array when the caller
//! supplies one, so the executor's tool-call loop runs over this client.
//! Streaming is not supported. The client holds only the gateway's URL and the
//! shared key; the vendor credential lives in the gateway, so the executor
//! never sees it. Point `PROMPTFORGE_GATEWAY_URL` at a local server or another
//! gateway to retarget it.

use std::fmt;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::dialects::{DialectRequest, ToolDialectRegistry};
use crate::model::{CompletionError, CompletionOptions};
use crate::{Error, Result};

/// A single chat message.
///
/// A plain `user` message serializes to just `{"role":..,"content":..}`; the
/// optional `tool_call_id` and `tool_calls` fields are emitted only when set,
/// which keeps the wire shape of ordinary messages unchanged.
// `PartialEq` compares messages structurally (F9). No `Eq`: `tool_calls` is an
// `Option<Vec<serde_json::Value>>`, and `Value` is not `Eq`, so a total
// equivalence cannot be honored.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
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
// `PartialEq` compares schemas structurally (F9). No `Eq`: `parameters` is a
// `serde_json::Value`, which is not `Eq`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ToolSchemaError {
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
    /// # Errors
    /// Returns [`ToolSchemaError::InvalidName`] when `name` is empty or contains
    /// a character outside `[A-Za-z0-9_.-]`, and
    /// [`ToolSchemaError::NonObjectSchema`] when `parameters` is not a JSON
    /// object, so a tool can never be advertised to the model with an unusable
    /// name or a non-object JSON Schema (F7).
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::client::ToolSchema;
    ///
    /// let schema = ToolSchema::new(
    ///     "search",
    ///     "Search the web",
    ///     serde_json::json!({"type": "object", "properties": {}}),
    /// )?;
    /// assert!(ToolSchema::new("", "d", serde_json::json!({})).is_err());
    /// assert!(ToolSchema::new("ok", "d", serde_json::json!([])).is_err());
    /// # Ok::<(), promptforge_core::client::ToolSchemaError>(())
    /// ```
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
#[derive(Debug, Clone, PartialEq)]
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
/// `Eq` is intentionally omitted: [`ToolCall`] arguments are a
/// [`serde_json::Value`], which is not `Eq`, so only `Clone` and `PartialEq`
/// are coherent.
#[derive(Debug, Clone, PartialEq)]
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

/// A bearer credential whose contents never appear in `Debug`, `Display`, or
/// logs.
///
/// Wrap any secret (the gateway bearer key) in a `SecretString` at the boundary
/// so an accidental `{:?}` or log line cannot leak it; only crate-internal
/// transport code reads the exposed value to set the `Authorization` header.
#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a non-empty secret so it is redacted everywhere it is formatted.
    ///
    /// # Errors
    /// Returns [`SecretError::Empty`] when `secret` is empty (F12), so a client
    /// can never be built to authenticate with a blank bearer credential.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::client::SecretString;
    ///
    /// let secret = SecretString::new("bearer-token")?;
    /// assert_eq!(format!("{secret:?}"), "SecretString(<redacted>)");
    /// assert_eq!(format!("{secret}"), "<redacted>");
    /// assert!(SecretString::new("").is_err());
    /// # Ok::<(), promptforge_core::client::SecretError>(())
    /// ```
    pub fn new(secret: impl Into<String>) -> std::result::Result<SecretString, SecretError> {
        let secret = secret.into();
        if secret.is_empty() {
            return Err(SecretError::Empty);
        }
        Ok(SecretString(secret))
    }

    /// Builds the empty sentinel used only by the disabled client, which never
    /// sends the credential. Crate-internal so no real credential path can
    /// produce a blank secret.
    pub(crate) fn disabled_placeholder() -> SecretString {
        SecretString(String::new())
    }

    /// Borrows the raw secret. Crate-internal so no downstream code can read a
    /// credential back out of the type.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

/// The reason a [`SecretString`] could not be constructed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SecretError {
    /// The supplied credential was empty.
    #[error("secret must not be empty")]
    Empty,
}

impl From<SecretError> for CompletionError {
    fn from(error: SecretError) -> CompletionError {
        // Classifies as `Config`: an unusable credential is a client
        // configuration problem, not a transport or backend failure.
        CompletionError::from(Error::MissingEnv(error.to_string()))
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// A validated gateway API base URL (the OpenAI-shaped `/v1` root).
///
/// Construction rejects a URL without an `http`/`https` scheme or host, so a
/// client can never be pointed at an unusable endpoint. A trailing slash is
/// trimmed so request paths join cleanly.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayEndpoint {
    url: String,
}

impl GatewayEndpoint {
    /// Validates and normalizes a gateway base URL.
    ///
    /// # Errors
    /// Returns a `Config`-kind [`CompletionError`] when `url` is not a valid
    /// absolute URL, does not use an `http`/`https` scheme, names no host,
    /// embeds credentials (a `user:pass@` component), or carries a query or
    /// fragment (an API root is a bare path). Parsing goes through a strict URL
    /// type (F12) rather than a hand-rolled prefix/host scan.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::client::GatewayEndpoint;
    ///
    /// let endpoint = GatewayEndpoint::new("https://gateway.example.com/v1/")?;
    /// assert_eq!(endpoint.url(), "https://gateway.example.com/v1");
    /// assert!(GatewayEndpoint::new("ftp://example.com").is_err());
    /// assert!(GatewayEndpoint::new("http://user:pass@host/v1").is_err());
    /// # Ok::<(), promptforge_core::model::CompletionError>(())
    /// ```
    pub fn new(url: &str) -> std::result::Result<GatewayEndpoint, CompletionError> {
        let reject = |detail: String| CompletionError::from(Error::MissingEnv(detail));
        let trimmed = url.trim();
        let parsed = url::Url::parse(trimmed)
            .map_err(|error| reject(format!("gateway URL is not a valid URL: {error}")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(reject(format!(
                "gateway URL must use the http or https scheme: {trimmed:?}"
            )));
        }
        match parsed.host_str() {
            None | Some("") => {
                return Err(reject(format!("gateway URL names no host: {trimmed:?}")));
            }
            Some(_) => {}
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(reject(
                "gateway URL must not embed credentials (user:pass@)".to_owned(),
            ));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(reject(
                "gateway URL must not carry a query or fragment".to_owned(),
            ));
        }
        Ok(GatewayEndpoint {
            // Normalized by the URL parser; trim the trailing slash so request
            // paths (`{base}/chat/completions`) join cleanly.
            url: parsed.as_str().trim_end_matches('/').to_string(),
        })
    }

    /// Returns the normalized base URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl TryFrom<&str> for GatewayEndpoint {
    type Error = CompletionError;

    fn try_from(url: &str) -> std::result::Result<GatewayEndpoint, CompletionError> {
        GatewayEndpoint::new(url)
    }
}

/// A chat completions client bound to one gateway URL and shared bearer key.
#[derive(Clone)]
pub struct GatewayClient {
    transport: GatewayTransport,
    base_url: String,
    key: SecretString,
    dialect_registry: Arc<ToolDialectRegistry>,
    /// Wall-clock cap applied to each completion request.
    request_timeout: Duration,
    /// Byte ceiling enforced on a response body before it is decoded.
    max_response_bytes: u64,
}

/// Default per-request timeout, matching [`crate::execute::RunLimits`].
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Default response-body ceiling, matching [`crate::execute::RunLimits`].
const DEFAULT_MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone)]
enum GatewayTransport {
    Http(reqwest::Client),
    Disabled,
}

impl fmt::Debug for GatewayClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The bearer key is a credential and must never appear in Debug output,
        // logs, or panic messages. It is redacted to a fixed marker regardless of
        // whether one is set, so no length or presence signal leaks either.
        f.debug_struct("GatewayClient")
            .field("base_url", &self.base_url)
            .field("key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl GatewayClient {
    /// Build a client from a validated [`GatewayEndpoint`] and a redacted
    /// [`SecretString`] bearer key (used by tests and by
    /// [`GatewayClient::from_env`]). Responses are parsed through the resolved
    /// tool dialect.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), promptforge_core::model::CompletionError> {
    /// use promptforge_core::client::{GatewayClient, GatewayEndpoint, Message, SecretString};
    /// use promptforge_core::model::CompletionOptions;
    /// use promptforge_core::dialects::ToolDialectId;
    ///
    /// let client = GatewayClient::new(
    ///     GatewayEndpoint::new("http://127.0.0.1:8081/v1")?,
    ///     SecretString::new("bearer-token").expect("non-empty key"),
    /// );
    /// let options = CompletionOptions::new("analyst", ToolDialectId::OpenAi);
    /// let completion = client
    ///     .complete(&[Message::user("hello")], None, &options)
    ///     .await?;
    /// let _ = completion.result();
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn new(endpoint: GatewayEndpoint, key: SecretString) -> GatewayClient {
        GatewayClient {
            transport: GatewayTransport::Http(reqwest::Client::new()),
            base_url: endpoint.url,
            key,
            dialect_registry: Arc::new(ToolDialectRegistry::builtin()),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    /// Build a client that cannot read gateway configuration or send HTTP.
    ///
    /// Hosts use this explicit sentinel for hermetic execution paths. Any
    /// attempted model call fails with a `Disabled`-kind [`CompletionError`].
    ///
    /// # Examples
    ///
    /// ```
    /// # async fn run() {
    /// use promptforge_core::client::{GatewayClient, Message};
    /// use promptforge_core::model::{CompletionErrorKind, CompletionOptions};
    /// use promptforge_core::dialects::ToolDialectId;
    ///
    /// let client = GatewayClient::disabled();
    /// let options = CompletionOptions::new("m", ToolDialectId::OpenAi);
    /// let error = client
    ///     .complete(&[Message::user("hi")], None, &options)
    ///     .await
    ///     .expect_err("a disabled client cannot complete");
    /// assert_eq!(error.kind(), CompletionErrorKind::Disabled);
    /// # }
    /// ```
    #[must_use]
    pub fn disabled() -> GatewayClient {
        GatewayClient {
            transport: GatewayTransport::Disabled,
            base_url: String::new(),
            key: SecretString::disabled_placeholder(),
            dialect_registry: Arc::new(ToolDialectRegistry::builtin()),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    /// Applies the run's HTTP limits to this client.
    ///
    /// Each completion request is bounded by `request_timeout`, and the response
    /// body is refused once it would exceed `max_response_bytes` before any
    /// UTF-8 or JSON decoding runs.
    #[must_use]
    pub fn with_request_limits(
        mut self,
        request_timeout: Duration,
        max_response_bytes: NonZeroU64,
    ) -> GatewayClient {
        self.request_timeout = request_timeout;
        self.max_response_bytes = max_response_bytes.get();
        self
    }

    /// Build a client from the environment.
    ///
    /// - URL: `PROMPTFORGE_GATEWAY_URL`. Required.
    /// - Key: `PROMPTFORGE_GATEWAY_KEY`, the gateway's shared bearer. Required.
    ///
    /// # Errors
    /// Returns a [`CompletionError`] with `Config` kind when either variable is
    /// not set.
    pub fn from_env() -> std::result::Result<GatewayClient, CompletionError> {
        from_env_with(|name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            // A set-but-non-Unicode value is a real misconfiguration, surfaced
            // explicitly instead of being silently treated as "not set".
            Err(std::env::VarError::NotUnicode(_)) => Err(Error::InvalidEnv(name.to_owned())),
        })
        .map_err(CompletionError::from)
    }

    /// Send a list of messages and return the model's outcome.
    ///
    /// When `tools` is `Some` and non-empty, each schema is wrapped into the
    /// `OpenAI` function shape and sent as the request's `tools` array (with
    /// `tool_choice` set to `auto`); passing `None` or an empty slice sends no
    /// `tools` field, preserving the plain chat-completions behavior.
    ///
    /// `options.model` names the model on the wire. Optional `temperature`,
    /// `max_tokens`, and `thinking` extend the request when present.
    ///
    /// # Errors
    /// Returns a [`CompletionError`] whose [`kind`](CompletionError::kind) is
    /// (F11 - the full reachable set):
    /// - `Disabled` when this client was built with [`GatewayClient::disabled`];
    /// - `Config` when the selected tool dialect is unknown, or dialect request
    ///   preparation fails;
    /// - `Transport` on a transport-layer failure (connection, timeout);
    /// - `Backend` when the gateway responds with a non-success status;
    /// - `MalformedResponse` when the body exceeds the size cap or its shape is
    ///   unusable (the JSON decode failure is retained as a private `#[source]`);
    /// - `EmptyReply` when the turn has neither non-empty tool calls nor
    ///   non-empty text.
    pub async fn complete(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        options: &CompletionOptions,
    ) -> std::result::Result<Completion, CompletionError> {
        let GatewayTransport::Http(http) = &self.transport else {
            return Err(CompletionError::from(Error::GatewayDisabled));
        };
        let mut request_body = serde_json::json!({
            "model": options.model,
            "messages": messages,
        });
        if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
            let wrapped: Vec<Value> = tools
                .iter()
                .map(|tool| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        },
                    })
                })
                .collect();
            request_body["tools"] = Value::Array(wrapped);
            request_body["tool_choice"] = Value::String("auto".into());
        }
        if let Some(temperature) = options.temperature {
            request_body["temperature"] = serde_json::json!(temperature.get());
        }
        if let Some(max_tokens) = options.max_tokens {
            request_body["max_tokens"] = serde_json::json!(max_tokens.get());
        }
        if let Some(thinking) = options.thinking {
            request_body["chat_template_kwargs"] = serde_json::json!({
                "enable_thinking": thinking,
            });
        }

        let dialect = self
            .dialect_registry
            .get(options.tool_dialect)
            .ok_or(Error::UnknownDialect(options.tool_dialect))?;
        let mut dr = DialectRequest::new(&mut request_body);
        dialect.prepare_request(&mut dr)?;

        let response = http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(self.key.expose())
            .timeout(self.request_timeout)
            .json(&request_body)
            .send()
            .await
            .map_err(Error::http)?;

        let status = response.status();
        let raw_body = read_body_capped(response, self.max_response_bytes).await?;
        if !status.is_success() {
            // F5: bound the body, then escape control characters so a hostile
            // payload cannot forge log lines. The escaped body is kept only for
            // the opt-in `CompletionError::backend_body` accessor, never the
            // public `Display`.
            let body = String::from_utf8_lossy(&raw_body);
            let body = escape_controls(&body, 2000);
            return Err(CompletionError::from(Error::Backend {
                status: status.as_u16(),
                body,
            }));
        }

        let response_body: Value = serde_json::from_slice(&raw_body).map_err(|error| {
            // F11 / MODEL-009: retain the decode failure as a private `#[source]`
            // cause rather than flattening it into the message string.
            Error::MalformedResponseSource {
                message: "completion response was not valid JSON".to_owned(),
                source: Box::new(error),
            }
        })?;
        let turn = dialect.parse_turn(&response_body)?;
        Ok(Completion {
            result: turn.outcome,
            finish_reason: turn.finish_reason,
            reasoning_content: turn.reasoning_content,
            request_body,
            response_body,
        })
    }
}

/// Escapes control characters in a diagnostic body and bounds it to `max` chars.
///
/// Control characters (including newlines and carriage returns) are rendered in
/// their `\u{..}`/`\n` escaped form so a backend body cannot forge log lines or
/// smuggle terminal control sequences into a diagnostic (F5). An empty body is
/// reported as a fixed marker.
fn escape_controls(body: &str, max: usize) -> String {
    if body.is_empty() {
        return "(empty body)".to_owned();
    }
    let mut escaped = String::with_capacity(body.len());
    for ch in body.chars().take(max) {
        if ch.is_control() {
            for part in ch.escape_default() {
                escaped.push(part);
            }
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

/// Reads a response body, refusing it once it would exceed `cap` bytes.
///
/// The advertised `Content-Length` short-circuits an oversize body, and the
/// streamed chunks are bounded so a gateway that omits or lies about the length
/// still cannot force an unbounded allocation before decoding.
async fn read_body_capped(mut response: reqwest::Response, cap: u64) -> Result<Vec<u8>> {
    if let Some(len) = response.content_length()
        && len > cap
    {
        return Err(Error::MalformedResponse(format!(
            "response body of {len} bytes exceeds the {cap}-byte limit"
        )));
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Error::http)? {
        if body.len() as u64 + chunk.len() as u64 > cap {
            return Err(Error::MalformedResponse(format!(
                "response body exceeds the {cap}-byte limit"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn from_env_with(
    lookup: impl Fn(&str) -> std::result::Result<Option<String>, Error>,
) -> Result<GatewayClient> {
    let base_url = lookup("PROMPTFORGE_GATEWAY_URL")?
        .ok_or_else(|| Error::MissingEnv("PROMPTFORGE_GATEWAY_URL".into()))?;
    let key = lookup("PROMPTFORGE_GATEWAY_KEY")?
        .ok_or_else(|| Error::MissingEnv("PROMPTFORGE_GATEWAY_KEY".into()))?;
    let endpoint = GatewayEndpoint::new(&base_url).map_err(Error::from)?;
    let key = SecretString::new(key)
        .map_err(|_| Error::MissingEnv("PROMPTFORGE_GATEWAY_KEY must not be empty".into()))?;
    Ok(GatewayClient::new(endpoint, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup_from<'a>(
        pairs: &'a [(&'a str, &'a str)],
    ) -> impl Fn(&str) -> std::result::Result<Option<String>, Error> + 'a {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name| {
            Ok(pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone()))
        }
    }

    #[test]
    fn from_env_surfaces_non_unicode_value_instead_of_dropping_it() {
        let err = from_env_with(|name| {
            if name == "PROMPTFORGE_GATEWAY_URL" {
                Err(Error::InvalidEnv(name.to_owned()))
            } else {
                Ok(Some("tok".to_owned()))
            }
        })
        .expect_err("a non-Unicode variable must be surfaced, not treated as missing");
        assert!(
            matches!(err, Error::InvalidEnv(ref name) if name == "PROMPTFORGE_GATEWAY_URL"),
            "expected an explicit InvalidEnv error, got {err:?}"
        );
    }

    #[test]
    fn from_env_missing_gateway_url() {
        let err = from_env_with(lookup_from(&[("PROMPTFORGE_GATEWAY_KEY", "tok")]))
            .expect_err("missing URL must fail");
        assert!(matches!(
            err,
            Error::MissingEnv(name) if name == "PROMPTFORGE_GATEWAY_URL"
        ));
    }

    #[test]
    fn from_env_missing_gateway_key() {
        let err = from_env_with(lookup_from(&[(
            "PROMPTFORGE_GATEWAY_URL",
            "http://127.0.0.1:8081/v1",
        )]))
        .expect_err("missing key must fail");
        assert!(matches!(
            err,
            Error::MissingEnv(name) if name == "PROMPTFORGE_GATEWAY_KEY"
        ));
    }

    #[test]
    fn debug_redacts_the_bearer_key_and_never_leaks_it() {
        let client = GatewayClient::new(
            GatewayEndpoint::new("http://127.0.0.1:8081/v1").expect("valid test endpoint"),
            SecretString::new("super-secret-token").expect("non-empty test key"),
        );
        let rendered = format!("{client:?}");
        assert!(
            !rendered.contains("super-secret-token"),
            "the bearer key must never appear in Debug output, got: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "the key field must be redacted, got: {rendered}"
        );
        assert!(
            rendered.contains("http://127.0.0.1:8081/v1"),
            "the base URL is not a secret and should still appear, got: {rendered}"
        );
    }

    #[test]
    fn secret_string_never_prints_its_contents() {
        let secret = SecretString::new("super-secret-token").expect("non-empty test key");
        assert_eq!(format!("{secret:?}"), "SecretString(<redacted>)");
        assert_eq!(format!("{secret}"), "<redacted>");
        assert_eq!(secret.expose(), "super-secret-token");
    }

    #[test]
    fn tool_arguments_view_exposes_no_raw_value() {
        // F8: the public arguments view surfaces typed accessors, never a
        // serde_json::Value.
        let call = ToolCall {
            id: "call_1".to_owned(),
            name: "search".to_owned(),
            arguments: serde_json::json!({"query": "rust", "limit": 5}),
        };
        let args = call.arguments();
        assert!(!args.is_empty());
        assert!(args.contains("query"));
        assert!(!args.contains("absent"));
        let mut names: Vec<_> = args.names().collect();
        names.sort_unstable();
        assert_eq!(names, ["limit", "query"]);
        let json = args.to_json_string();
        assert!(json.contains("\"query\":\"rust\""), "got {json}");

        // A null payload reads as empty.
        let empty = ToolCall {
            id: "c2".to_owned(),
            name: "noop".to_owned(),
            arguments: Value::Null,
        };
        assert!(empty.arguments().is_empty());
        assert!(!empty.arguments().contains("anything"));
    }

    #[test]
    fn tool_schema_new_validates_wire_name_and_object_schema() {
        // F7: a valid name and object schema are accepted.
        let schema = ToolSchema::new("web.search-1", "desc", json_object())
            .expect("a valid schema is accepted");
        assert_eq!(schema.name, "web.search-1");
        // An empty or malformed name is rejected.
        assert!(matches!(
            ToolSchema::new("", "d", json_object()),
            Err(ToolSchemaError::InvalidName { .. })
        ));
        assert!(matches!(
            ToolSchema::new("bad name", "d", json_object()),
            Err(ToolSchemaError::InvalidName { .. })
        ));
        // A non-object JSON Schema is rejected.
        assert!(matches!(
            ToolSchema::new("ok", "d", serde_json::json!([1, 2, 3])),
            Err(ToolSchemaError::NonObjectSchema { .. })
        ));
        assert!(matches!(
            ToolSchema::new("ok", "d", serde_json::json!("scalar")),
            Err(ToolSchemaError::NonObjectSchema { .. })
        ));
    }

    fn json_object() -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    #[test]
    fn escape_controls_neutralizes_control_bytes_and_bounds_length() {
        // F5: newlines and other control characters are escaped, not passed
        // through, so a body cannot forge log lines.
        let escaped = escape_controls("line1\nline2\r\u{7}end", 2000);
        assert!(!escaped.contains('\n'), "raw newline must be escaped");
        assert!(
            !escaped.contains('\r'),
            "raw carriage return must be escaped"
        );
        assert!(
            escaped.contains("\\n"),
            "escaped newline expected, got {escaped}"
        );
        assert_eq!(escape_controls("", 2000), "(empty body)");
        assert_eq!(escape_controls("abcdef", 3), "abc");
    }

    #[tokio::test]
    async fn backend_error_display_is_body_free_and_body_is_opt_in_and_escaped() {
        use axum::Router;
        use axum::routing::post;
        use tokio::net::TcpListener;

        // A non-success body carrying control characters and a would-be secret.
        async fn handler() -> (axum::http::StatusCode, String) {
            (
                axum::http::StatusCode::BAD_GATEWAY,
                "forged\nlog: super-secret".to_owned(),
            )
        }
        let app = Router::new().route("/v1/chat/completions", post(handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = GatewayClient::new(
            GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid endpoint"),
            SecretString::new("tok").expect("non-empty test key"),
        );
        let options = CompletionOptions::new("m", crate::dialects::ToolDialectId::OpenAi);
        let err = client
            .complete(&[Message::user("hi")], None, &options)
            .await
            .expect_err("a 502 must surface as a backend error");

        // F5: the public Display names only the status, never the raw body.
        let shown = err.to_string();
        assert!(shown.contains("502"), "status must appear, got {shown}");
        assert!(
            !shown.contains("super-secret") && !shown.contains('\n'),
            "the raw body must not ride in Display, got {shown}"
        );
        // The bounded, control-escaped body is available only via the opt-in.
        let body = err
            .backend_body()
            .expect("backend body is available opt-in");
        assert!(
            body.contains("\\n"),
            "control chars must be escaped, got {body}"
        );
        assert!(
            !body.contains('\n'),
            "no raw newline in the diagnostic body"
        );
    }

    #[test]
    fn gateway_endpoint_rejects_non_http_schemes_and_missing_host() {
        assert!(GatewayEndpoint::new("ftp://example.com/v1").is_err());
        assert!(GatewayEndpoint::new("not-a-url").is_err());
        assert!(GatewayEndpoint::new("http://").is_err());
        assert!(GatewayEndpoint::new("").is_err());
    }

    #[test]
    fn gateway_endpoint_rejects_credentials_query_and_fragment() {
        // F12: the strict URL parse rejects embedded credentials and the
        // query/fragment ambiguity a hand-rolled prefix scan let through.
        assert!(GatewayEndpoint::new("http://user:pass@host/v1").is_err());
        assert!(GatewayEndpoint::new("http://user@host/v1").is_err());
        assert!(GatewayEndpoint::new("http://host/v1?token=leak").is_err());
        assert!(GatewayEndpoint::new("http://host/v1#frag").is_err());
        // A clean http(s) API root is still accepted and normalized.
        assert_eq!(
            GatewayEndpoint::new("http://host:8080/v1/")
                .expect("clean URL")
                .url(),
            "http://host:8080/v1"
        );
    }

    #[test]
    fn secret_string_construction_rejects_an_empty_credential() {
        // F12: an empty bearer credential is unrepresentable.
        assert!(matches!(SecretString::new(""), Err(SecretError::Empty)));
        assert!(SecretString::new("tok").is_ok());
    }

    #[test]
    fn gateway_endpoint_trims_trailing_slash_and_keeps_valid_urls() {
        let endpoint = GatewayEndpoint::new("https://gateway.example.com/v1/")
            .expect("a well-formed https URL is accepted");
        assert_eq!(endpoint.url(), "https://gateway.example.com/v1");
    }

    #[tokio::test]
    async fn complete_sends_completion_options_model_on_the_wire() {
        use std::sync::{Arc, Mutex};

        use axum::Router;
        use axum::extract::Json;
        use axum::routing::post;
        use serde_json::{Value, json};
        use tokio::net::TcpListener;

        let captured: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&captured);
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let slot = Arc::clone(&slot);
                async move {
                    *slot.lock().expect("capture lock") = Some(body);
                    Json(json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "ok" },
                            "finish_reason": "stop"
                        }]
                    }))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = GatewayClient::new(
            GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
            SecretString::new("tok").expect("non-empty test key"),
        );
        let options = CompletionOptions {
            model: "analyst".into(),
            temperature: Some(crate::model::Temperature::new(0.0).expect("0.0 is valid")),
            max_tokens: Some(std::num::NonZeroU32::new(128).expect("128 is non-zero")),
            thinking: Some(false),
            tool_dialect: crate::dialects::ToolDialectId::OpenAi,
        };
        client
            .complete(&[Message::user("hi")], None, &options)
            .await
            .unwrap();
        let body = captured.lock().expect("capture lock").clone().unwrap();
        assert_eq!(body["model"], "analyst");
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
    }

    #[tokio::test]
    async fn complete_hard_fails_on_empty_model_reply() {
        use axum::Router;
        use axum::extract::Json;
        use axum::routing::post;
        use serde_json::{Value, json};
        use tokio::net::TcpListener;

        let app = Router::new().route(
            "/v1/chat/completions",
            post(|Json(_body): Json<Value>| async move {
                Json(json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": "ignored"
                        },
                        "finish_reason": "stop"
                    }]
                }))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = GatewayClient::new(
            GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
            SecretString::new("tok").expect("non-empty test key"),
        );
        let options = CompletionOptions {
            model: "m".into(),
            temperature: None,
            max_tokens: None,
            thinking: None,
            tool_dialect: crate::dialects::ToolDialectId::OpenAi,
        };
        let err = client
            .complete(&[Message::user("hi")], None, &options)
            .await
            .expect_err("empty product must fail");
        assert!(matches!(Error::from(err), Error::EmptyModelReply { .. }));
    }

    fn openai_options() -> CompletionOptions {
        CompletionOptions::new("m", crate::dialects::ToolDialectId::OpenAi)
    }

    /// Spawns a gateway that answers `/v1/chat/completions` with a fixed status
    /// and raw body, returning its address.
    async fn spawn_raw_gateway(status: axum::http::StatusCode, body: &'static str) -> String {
        use axum::Router;
        use axum::routing::post;
        use tokio::net::TcpListener;

        let app = Router::new().route(
            "/v1/chat/completions",
            post(move || async move { (status, body) }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}/v1")
    }

    #[tokio::test]
    async fn complete_on_a_disabled_client_is_a_disabled_error() {
        // F14: a disabled client never touches the network.
        let client = GatewayClient::disabled();
        let err = client
            .complete(&[Message::user("hi")], None, &openai_options())
            .await
            .expect_err("a disabled client cannot complete");
        assert_eq!(err.kind(), crate::model::CompletionErrorKind::Disabled);
    }

    #[tokio::test]
    async fn complete_refuses_a_success_body_over_the_size_cap() {
        // F14 (body-size, success path): a 200 body larger than the cap is
        // refused before decoding.
        let base = spawn_raw_gateway(
            axum::http::StatusCode::OK,
            "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"a long reply\"}}]}",
        )
        .await;
        let client = GatewayClient::new(
            GatewayEndpoint::new(&base).expect("valid endpoint"),
            SecretString::new("tok").expect("non-empty test key"),
        )
        .with_request_limits(
            DEFAULT_REQUEST_TIMEOUT,
            NonZeroU64::new(8).expect("non-zero cap"),
        );
        let err = client
            .complete(&[Message::user("hi")], None, &openai_options())
            .await
            .expect_err("an oversize body must be refused");
        assert_eq!(
            err.kind(),
            crate::model::CompletionErrorKind::MalformedResponse
        );
    }

    #[tokio::test]
    async fn complete_refuses_a_backend_error_body_over_the_size_cap() {
        // F14 (body-size, error path): a non-success body larger than the cap is
        // also refused before it is buffered.
        let base = spawn_raw_gateway(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "this backend error body is definitely longer than eight bytes",
        )
        .await;
        let client = GatewayClient::new(
            GatewayEndpoint::new(&base).expect("valid endpoint"),
            SecretString::new("tok").expect("non-empty test key"),
        )
        .with_request_limits(
            DEFAULT_REQUEST_TIMEOUT,
            NonZeroU64::new(8).expect("non-zero cap"),
        );
        let err = client
            .complete(&[Message::user("hi")], None, &openai_options())
            .await
            .expect_err("an oversize error body must be refused");
        assert_eq!(
            err.kind(),
            crate::model::CompletionErrorKind::MalformedResponse
        );
    }

    #[tokio::test]
    async fn complete_refuses_malformed_successful_json() {
        // F14: a 200 whose body is not valid JSON is MalformedResponse, and the
        // decode failure is preserved as the error-chain source.
        let base = spawn_raw_gateway(axum::http::StatusCode::OK, "{ not json").await;
        let client = GatewayClient::new(
            GatewayEndpoint::new(&base).expect("valid endpoint"),
            SecretString::new("tok").expect("non-empty test key"),
        );
        let err = client
            .complete(&[Message::user("hi")], None, &openai_options())
            .await
            .expect_err("undecodable body must fail");
        assert_eq!(
            err.kind(),
            crate::model::CompletionErrorKind::MalformedResponse
        );
        let source =
            std::error::Error::source(&err).expect("the decode error must be a preserved source");
        assert!(
            source.downcast_ref::<serde_json::Error>().is_some(),
            "the preserved source must be the JSON decode error, got {source}"
        );
    }

    #[tokio::test]
    async fn complete_refuses_malformed_tool_call_arguments_at_the_boundary() {
        // F14: a well-formed HTTP 200 whose tool-call arguments are not a JSON
        // object string is rejected at the client boundary, not passed on.
        let base = spawn_raw_gateway(
            axum::http::StatusCode::OK,
            "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":null,\
             \"tool_calls\":[{\"id\":\"c1\",\"type\":\"function\",\
             \"function\":{\"name\":\"t\",\"arguments\":123}}]},\
             \"finish_reason\":\"tool_calls\"}]}",
        )
        .await;
        let client = GatewayClient::new(
            GatewayEndpoint::new(&base).expect("valid endpoint"),
            SecretString::new("tok").expect("non-empty test key"),
        );
        let err = client
            .complete(&[Message::user("hi")], None, &openai_options())
            .await
            .expect_err("malformed tool arguments must be rejected");
        assert_eq!(
            err.kind(),
            crate::model::CompletionErrorKind::MalformedResponse
        );
    }
}
