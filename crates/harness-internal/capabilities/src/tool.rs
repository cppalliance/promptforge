//! The [`Tool`] trait: the implementation contract behind a
//! [`ToolDescriptor`] the engine binds against.
//!
//! The engine's catalog is descriptors
//! ([`ToolCatalog`](promptforge::tools::ToolCatalog)), and a
//! `ToolCall` effect names a [`ToolId`]; the harness resolves the id in
//! its [`ToolTable`](crate::ToolTable) and calls the implementation here.
//! Some tools run locally in the harness process (fetching and rendering a
//! web page), others proxy through the gateway so a shared credential never
//! leaves the server; both share this trait so the tool performer dispatches
//! them uniformly.

use promptforge::tools::{ToolDescriptor, ToolError, ToolId, ToolOutput};

#[cfg(test)]
#[path = "tool-tests.rs"]
mod tests;

/// A tool the harness can dispatch during a model's tool-call loop.
///
/// # Implementing
///
/// A complete implementation supplies a stable identity, a transport wire name,
/// a model-facing description, a JSON-Schema parameter object, and an async
/// [`call`](Tool::call). A minimal doctested implementation:
///
/// ```
/// use harness_capabilities::Tool;
/// use promptforge::tools::{
///     OutputTrust, ToolError, ToolErrorKind, ToolId, ToolOutput,
/// };
///
/// struct Echo {
///     id: ToolId,
/// }
///
/// #[async_trait::async_trait]
/// impl Tool for Echo {
///     fn id(&self) -> ToolId {
///         // The identity is validated once at construction, so this accessor
///         // is infallible and never panics.
///         self.id.clone()
///     }
///     fn wire_name(&self) -> &str {
///         "echo"
///     }
///     fn description(&self) -> &str {
///         "Echo the `text` argument back to the model."
///     }
///     fn parameters_schema(&self) -> serde_json::Value {
///         serde_json::json!({
///             "type": "object",
///             "properties": { "text": { "type": "string" } },
///             "required": ["text"],
///         })
///     }
///     async fn call(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError> {
///         let text = args.get("text").and_then(serde_json::Value::as_str).ok_or_else(|| {
///             ToolError::message("echo: missing string `text`")
///                 .with_kind(ToolErrorKind::InvalidArguments)
///         })?;
///         // First-party, non-attacker content: trusted.
///         Ok(ToolOutput::trusted(text.to_owned()))
///     }
/// }
///
/// let echo = Echo { id: ToolId::parse("example/echo/echo")? };
/// assert_eq!(echo.wire_name(), "echo");
/// assert_eq!(echo.id().name(), "echo");
/// assert_eq!(echo.descriptor().wire_name, "echo");
/// # let _ = OutputTrust::Trusted;
/// # Ok::<(), promptforge::tools::ToolIdError>(())
/// ```
///
/// # Compatibility policy
///
/// This trait is a stable extension point and is deliberately open. Adding a
/// **new required** method (one without a default body) is a breaking change for
/// downstream implementers; new capabilities must therefore ship with a default
/// implementation. Existing method signatures are stable.
///
/// # Invariants
///
/// - [`id`](Tool::id) returns the same value on every call for a given tool; it
///   is the catalog key and must be unique within a catalog (whose entries are
///   the tool's [`ToolDescriptor`]).
/// - [`wire_name`](Tool::wire_name) is the transport name, not identity; it is
///   distinct from [`id`](Tool::id) and may be aliased when advertised.
/// - [`parameters_schema`](Tool::parameters_schema) returns a JSON-Schema
///   `object` describing the accepted [`call`](Tool::call) arguments.
/// - [`call`](Tool::call) is cancellation-aware, must not panic (a panic unwinds
///   the run), and must classify every failure trust-correctly: any output that
///   embeds attacker-influenceable data is [`ToolOutput::untrusted`].
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Returns the tool's stable live identity.
    ///
    /// This is the catalog key. It must be stable across calls and unique
    /// within any catalog the tool is described into.
    fn id(&self) -> ToolId;

    /// Returns the concrete name used by the current model transport.
    ///
    /// This is not the tool's identity. It may later be replaced by a
    /// prompt-local alias when the tool is advertised to a model. It should be a
    /// non-empty transport-legal token (no `/` separator or control characters).
    fn wire_name(&self) -> &str;

    /// A one-sentence description supplied to the model.
    fn description(&self) -> &str;

    /// The JSON Schema describing the tool's parameters.
    ///
    /// Returns a JSON-Schema `object` (a map with `"type": "object"` and a
    /// `properties` map) whose shape matches the arguments [`call`](Tool::call)
    /// accepts.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Whether [`call`](Tool::call) output is structured JSON rather than
    /// plain text.
    ///
    /// A structured tool's output text is one JSON value, and an executor
    /// that supports structured results resumes it into the script as data
    /// (for example, a Lua table) instead of a string. The default is
    /// `false`: plain text. Structured output is honored for trusted
    /// output only - an untrusted result is nonce-wrapped before any
    /// parse, so the wrapped text no longer parses as JSON and the call
    /// fails rather than smuggling attacker-influenceable data past the
    /// guard.
    fn structured_output(&self) -> bool {
        false
    }

    /// The tool as data: the descriptor the harness derives from this
    /// implementation when it assembles the run's catalog, with no
    /// conflicts recorded (the contributing capability's are added at
    /// assembly).
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor::new(
            self.id(),
            self.wire_name(),
            self.description(),
            self.parameters_schema(),
        )
        .structured(self.structured_output())
    }

    /// Executes the tool with the given JSON arguments and returns its output.
    ///
    /// The returned [`ToolOutput`] includes its own
    /// [`OutputTrust`](promptforge::tools::OutputTrust), so trust
    /// is mandatory and cannot be forgotten: an
    /// [`OutputTrust::Untrusted`](promptforge::tools::OutputTrust::Untrusted)
    /// result is nonce-wrapped before it can reach model input. A failure
    /// returns a narrow, model-safe [`ToolError`]. Implementations must not
    /// panic and should return promptly when the run is cancelled.
    ///
    /// # Errors
    /// Returns a [`ToolError`] if the arguments are unacceptable, the backend
    /// refuses, the transport fails, or the run is cancelled.
    async fn call(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError>;
}
