//! Tools the executor can dispatch during a model's tool-call loop.
//!
//! Some tools run locally in this process (for example fetching and rendering a
//! web page), while others proxy through the gateway so a shared credential
//! never leaves the server. Both kinds share the [`Tool`] trait so the executor
//! can dispatch them uniformly. Stable identity is separate from the wire name
//! used by the current model transport.

use std::sync::Arc;

mod web_search;

pub use web_search::WebSearch;

/// Cloneable tool set for concurrent fanout arms.
///
/// Each arm builds a short-lived [`ToolRegistry`] that borrows these `Arc`s.
#[derive(Clone, Default)]
pub(crate) struct SharedTools {
    tools: Arc<[Arc<dyn Tool>]>,
}

impl std::fmt::Debug for SharedTools {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedTools")
            .field(
                "ids",
                &self.tools.iter().map(|tool| tool.id()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl SharedTools {
    /// Builds a shared set from caller-owned tool arcs.
    ///
    /// Validates identity uniqueness once, here, so every [`Self::registry`] can
    /// trust the invariant without rescanning.
    ///
    /// # Errors
    /// Returns [`ToolRegistryError`] if two tools share a [`ToolId`].
    pub(crate) fn new(tools: &[Arc<dyn Tool>]) -> Result<Self, ToolRegistryError> {
        ToolRegistry::new(tools.iter().map(AsRef::as_ref))?;
        Ok(Self {
            tools: Arc::from(tools.to_vec()),
        })
    }

    /// Borrowing registry over the shared arcs.
    ///
    /// Uniqueness was established by [`Self::new`], so this skips revalidation.
    #[must_use]
    pub(crate) fn registry(&self) -> ToolRegistry<'_> {
        ToolRegistry::from_unique(self.tools.iter().map(AsRef::as_ref))
    }
}

/// The stable identity of a live tool.
///
/// Identity is structural over the server and tool name. The wire name used
/// in a model request is deliberately not identity: later capability binding
/// can advertise a selected tool under a prompt-local alias without changing
/// the live tool it dispatches.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ToolId {
    server: String,
    name: String,
}

impl ToolId {
    /// Builds an identity from its server and stable tool name.
    ///
    /// # Errors
    /// Returns [`ToolIdError`] if `server` or `name` is empty or contains the
    /// `/` namespace separator or a control character.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolId;
    ///
    /// let id = ToolId::new("promptforge", "web_fetch")?;
    /// assert_eq!(id.server(), "promptforge");
    /// assert_eq!(id.name(), "web_fetch");
    /// # Ok::<(), promptforge_core::tools::ToolIdError>(())
    /// ```
    pub fn new(server: impl Into<String>, name: impl Into<String>) -> Result<ToolId, ToolIdError> {
        let server = server.into();
        let name = name.into();
        Self::validate("server", &server)?;
        Self::validate("name", &name)?;
        Ok(Self { server, name })
    }

    /// Builds an identity from components already known to be valid.
    ///
    /// For internal callers whose inputs are static tool names or come from an
    /// existing [`ToolId`], so the validation in [`ToolId::new`] is redundant.
    pub(crate) fn from_validated(server: impl Into<String>, name: impl Into<String>) -> ToolId {
        ToolId {
            server: server.into(),
            name: name.into(),
        }
    }

    /// Validates one identity component, naming the field in any error.
    fn validate(field: &'static str, value: &str) -> Result<(), ToolIdError> {
        if value.is_empty() {
            return Err(ToolIdError {
                field,
                reason: "must not be empty",
            });
        }
        if value.contains('/') {
            return Err(ToolIdError {
                field,
                reason: "must not contain the '/' separator",
            });
        }
        if value.bytes().any(|b| b < 0x20 || b == 0x7f) {
            return Err(ToolIdError {
                field,
                reason: "must not contain a control character",
            });
        }
        Ok(())
    }

    /// Returns the server that owns this identity namespace.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolId;
    ///
    /// let id = ToolId::new("promptforge", "web_fetch").expect("valid id");
    /// assert_eq!(id.server(), "promptforge");
    /// ```
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Returns the stable name within the publisher's namespace.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolId;
    ///
    /// let id = ToolId::new("promptforge", "web_fetch").expect("valid id");
    /// assert_eq!(id.name(), "web_fetch");
    /// ```
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Whether a tool's output is trusted or must be treated as untrusted data.
///
/// Trust is mandatory and carried in [`ToolOutput`] so it cannot be forgotten:
/// an [`OutputTrust::Untrusted`] result is nonce-wrapped before it can reach
/// model input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputTrust {
    /// The output was produced by trusted, first-party code.
    Trusted,
    /// The output contains attacker-influenceable external data.
    Untrusted,
}

/// The result of a successful [`Tool::call`], carrying its text and trust.
///
/// Trust travels with the value so the executor never has to remember a
/// separate flag; construct with [`ToolOutput::trusted`] or
/// [`ToolOutput::untrusted`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolOutput {
    text: String,
    trust: OutputTrust,
}

impl ToolOutput {
    /// Builds a trusted output whose text is appended to the model verbatim.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{OutputTrust, ToolOutput};
    ///
    /// let out = ToolOutput::trusted("done");
    /// assert_eq!(out.trust(), OutputTrust::Trusted);
    /// assert_eq!(out.text(), "done");
    /// ```
    #[must_use]
    pub fn trusted(text: impl Into<String>) -> ToolOutput {
        ToolOutput {
            text: text.into(),
            trust: OutputTrust::Trusted,
        }
    }

    /// Builds an untrusted output that is nonce-wrapped before reaching a model.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{OutputTrust, ToolOutput};
    ///
    /// let out = ToolOutput::untrusted("<html>...");
    /// assert_eq!(out.trust(), OutputTrust::Untrusted);
    /// ```
    #[must_use]
    pub fn untrusted(text: impl Into<String>) -> ToolOutput {
        ToolOutput {
            text: text.into(),
            trust: OutputTrust::Untrusted,
        }
    }

    /// Borrows the output text.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::ToolOutput;
    ///
    /// assert_eq!(ToolOutput::trusted("hi").text(), "hi");
    /// ```
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns whether the output is trusted or untrusted.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{OutputTrust, ToolOutput};
    ///
    /// assert_eq!(ToolOutput::untrusted("x").trust(), OutputTrust::Untrusted);
    /// ```
    #[must_use]
    pub fn trust(&self) -> OutputTrust {
        self.trust
    }
}

/// A stable, matchable classification of a [`ToolError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolErrorKind {
    /// The model supplied arguments the tool could not accept.
    InvalidArguments,
    /// The tool's backend refused or failed the request.
    Backend,
    /// The request failed at the transport layer (network, timeout).
    Transport,
    /// The run was cancelled before or during the call.
    Cancelled,
    /// Any other tool failure.
    Other,
}

/// A narrow, model-safe error from a [`Tool::call`].
///
/// The `Display` message is caller-facing and safe to hand back to the model;
/// any underlying cause is hidden behind [`std::error::Error::source`]. Match on
/// [`ToolError::kind`] rather than a private representation.
#[derive(Debug)]
#[non_exhaustive]
pub struct ToolError {
    kind: ToolErrorKind,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl ToolError {
    /// Builds a model-safe error carrying only a message (kind `Other`).
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{ToolError, ToolErrorKind};
    ///
    /// let err = ToolError::message("could not read the page");
    /// assert_eq!(err.kind(), ToolErrorKind::Other);
    /// ```
    #[must_use]
    pub fn message(text: impl Into<String>) -> ToolError {
        ToolError {
            kind: ToolErrorKind::Other,
            message: text.into(),
            source: None,
        }
    }

    /// Builds a model-safe error that keeps `src` as a hidden `#[source]` cause.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::ToolError;
    ///
    /// let io = std::io::Error::other("boom");
    /// let err = ToolError::with_source("backend failed", io);
    /// assert!(std::error::Error::source(&err).is_some());
    /// ```
    #[must_use]
    pub fn with_source(
        text: impl Into<String>,
        src: impl std::error::Error + Send + Sync + 'static,
    ) -> ToolError {
        ToolError {
            kind: ToolErrorKind::Backend,
            message: text.into(),
            source: Some(Box::new(src)),
        }
    }

    /// Sets the classification, returning the updated error.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{ToolError, ToolErrorKind};
    ///
    /// let err = ToolError::message("bad args").with_kind(ToolErrorKind::InvalidArguments);
    /// assert_eq!(err.kind(), ToolErrorKind::InvalidArguments);
    /// ```
    #[must_use]
    pub fn with_kind(mut self, kind: ToolErrorKind) -> ToolError {
        self.kind = kind;
        self
    }

    /// Returns the stable classification of this error.
    #[must_use]
    pub fn kind(&self) -> ToolErrorKind {
        self.kind
    }

    /// Returns whether the failure was a cancellation.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{ToolError, ToolErrorKind};
    ///
    /// let err = ToolError::message("stopped").with_kind(ToolErrorKind::Cancelled);
    /// assert!(err.is_cancelled());
    /// ```
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self.kind, ToolErrorKind::Cancelled)
    }

    /// Returns whether retrying the same call could plausibly succeed.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{ToolError, ToolErrorKind};
    ///
    /// let err = ToolError::message("timeout").with_kind(ToolErrorKind::Transport);
    /// assert!(err.is_retryable());
    /// ```
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self.kind, ToolErrorKind::Transport)
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ToolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|boxed| boxed.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// The reason a [`ToolId`] could not be built from its components.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid tool id: {field} {reason}")]
#[non_exhaustive]
pub struct ToolIdError {
    /// Which component was rejected (`server` or `name`).
    field: &'static str,
    /// Why it was rejected.
    reason: &'static str,
}

/// A [`ToolRegistry`] could not be built because an identity was repeated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("duplicate tool identity {id:?} in registry")]
#[non_exhaustive]
pub struct ToolRegistryError {
    /// The stable identity supplied more than once.
    id: ToolId,
}

impl ToolRegistryError {
    /// Returns the stable identity that was supplied more than once.
    #[must_use]
    pub fn id(&self) -> &ToolId {
        &self.id
    }
}

impl From<ToolRegistryError> for crate::error::Error {
    fn from(error: ToolRegistryError) -> Self {
        crate::error::Error::DuplicateLiveToolId { id: error.id }
    }
}

/// A tool the executor can dispatch during a model's tool-call loop.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Returns the tool's stable live identity.
    fn id(&self) -> ToolId;

    /// Returns the concrete name used by the current model transport.
    ///
    /// This is not the tool's identity. It may later be replaced by a
    /// prompt-local alias when the tool is advertised to a model.
    fn wire_name(&self) -> &str;

    /// A one-sentence description supplied to the model.
    fn description(&self) -> &str;

    /// The JSON Schema describing the tool's parameters.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute the tool with the given JSON arguments and return its output.
    ///
    /// The returned [`ToolOutput`] carries its own [`OutputTrust`], so trust is
    /// mandatory and cannot be forgotten: an [`OutputTrust::Untrusted`] result
    /// is nonce-wrapped before it can reach model input. A failure returns a
    /// narrow, model-safe [`ToolError`].
    ///
    /// # Errors
    /// Returns a [`ToolError`] if the arguments are unacceptable, the backend
    /// refuses, the transport fails, or the run is cancelled.
    async fn call(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError>;
}

/// An ordered collection of callable live tools with unique identities.
///
/// Registration rejects repeated stable identities: a [`ToolRegistry`] can never
/// hold two tools that share a [`ToolId`]. Iteration preserves supplied order and
/// lookup is identity-based.
pub struct ToolRegistry<'a> {
    tools: Vec<&'a dyn Tool>,
}

impl std::fmt::Debug for ToolRegistry<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolRegistry")
            .field(
                "ids",
                &self.tools.iter().map(|tool| tool.id()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl<'a> ToolRegistry<'a> {
    /// Builds a registry in the order the live tools are supplied.
    ///
    /// # Errors
    /// Returns [`ToolRegistryError`] if two supplied tools share a [`ToolId`];
    /// the registry never holds duplicate identities.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolRegistry;
    ///
    /// let registry = ToolRegistry::new(std::iter::empty())?;
    /// assert!(registry.is_empty());
    /// # Ok::<(), promptforge_core::tools::ToolRegistryError>(())
    /// ```
    pub fn new(
        tools: impl IntoIterator<Item = &'a dyn Tool>,
    ) -> Result<ToolRegistry<'a>, ToolRegistryError> {
        let tools: Vec<&'a dyn Tool> = tools.into_iter().collect();
        let mut seen = std::collections::BTreeSet::new();
        for tool in &tools {
            let id = tool.id();
            if !seen.insert(id.clone()) {
                return Err(ToolRegistryError { id });
            }
        }
        Ok(Self { tools })
    }

    /// Builds a registry from tools whose identities are already known unique.
    ///
    /// For internal callers that validated uniqueness when the tool set was
    /// assembled (see [`SharedTools`]), avoiding a redundant second scan.
    pub(crate) fn from_unique(tools: impl IntoIterator<Item = &'a dyn Tool>) -> ToolRegistry<'a> {
        Self {
            tools: tools.into_iter().collect(),
        }
    }

    /// Returns the number of live registry entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolRegistry;
    ///
    /// assert_eq!(ToolRegistry::new(std::iter::empty())?.len(), 0);
    /// # Ok::<(), promptforge_core::tools::ToolRegistryError>(())
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Returns whether the registry has no live entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolRegistry;
    ///
    /// assert!(ToolRegistry::new(std::iter::empty())?.is_empty());
    /// # Ok::<(), promptforge_core::tools::ToolRegistryError>(())
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Returns the live entries in registry order.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::ToolRegistry;
    ///
    /// assert!(ToolRegistry::new(std::iter::empty())?.tools().is_empty());
    /// # Ok::<(), promptforge_core::tools::ToolRegistryError>(())
    /// ```
    #[must_use]
    pub fn tools(&self) -> &[&'a dyn Tool] {
        &self.tools
    }

    /// Returns the first live tool with `id`.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::tools::{ToolId, ToolRegistry};
    ///
    /// let registry = ToolRegistry::new(std::iter::empty())?;
    /// let missing = ToolId::new("promptforge", "missing").expect("valid id");
    /// assert!(registry.get(&missing).is_none());
    /// # Ok::<(), promptforge_core::tools::ToolRegistryError>(())
    /// ```
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&'a dyn Tool> {
        self.tools.iter().copied().find(|tool| tool.id() == *id)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Tool, ToolError, ToolId, ToolOutput, ToolRegistry};

    struct FixtureTool;

    #[async_trait::async_trait]
    impl Tool for FixtureTool {
        fn id(&self) -> ToolId {
            ToolId::new("fixtures", "inspect").expect("fixture id is valid")
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn wire_name(&self) -> &str {
            "inspect_wire"
        }

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "the Tool trait fixes this return type to &str"
        )]
        fn description(&self) -> &str {
            "Inspect a fixture."
        }

        fn parameters_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            })
        }

        async fn call(&self, _args: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::trusted(String::new()))
        }
    }

    struct RegistryFixtureTool {
        id_name: &'static str,
        wire_name: &'static str,
    }

    #[async_trait::async_trait]
    impl Tool for RegistryFixtureTool {
        fn id(&self) -> ToolId {
            ToolId::new("fixtures", self.id_name).expect("fixture id is valid")
        }

        fn wire_name(&self) -> &str {
            self.wire_name
        }

        fn description(&self) -> &str {
            self.wire_name
        }

        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn call(&self, _args: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::trusted(String::new()))
        }
    }

    #[test]
    fn trait_is_dyn_compatible() {
        let tools: Vec<Box<dyn Tool>> = Vec::new();
        assert!(tools.is_empty());
    }

    #[test]
    fn tool_output_carries_mandatory_trust() {
        use super::{OutputTrust, ToolOutput};
        assert_eq!(ToolOutput::trusted("a").trust(), OutputTrust::Trusted);
        assert_eq!(ToolOutput::untrusted("b").trust(), OutputTrust::Untrusted);
        assert_eq!(ToolOutput::trusted("a").text(), "a");
    }

    #[test]
    fn tool_error_classifies_and_hides_source() {
        use super::{ToolError, ToolErrorKind};
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<ToolError>();

        let plain = ToolError::message("model-safe");
        assert_eq!(plain.kind(), ToolErrorKind::Other);
        assert_eq!(plain.to_string(), "model-safe");
        assert!(!plain.is_cancelled() && !plain.is_retryable());

        let cancelled = ToolError::message("stopped").with_kind(ToolErrorKind::Cancelled);
        assert!(cancelled.is_cancelled());

        let retry = ToolError::message("net").with_kind(ToolErrorKind::Transport);
        assert!(retry.is_retryable());

        let sourced = ToolError::with_source("wrap", std::io::Error::other("cause"));
        assert!(std::error::Error::source(&sourced).is_some());
    }

    #[test]
    fn descriptor_surface_preserves_identity_description_and_schema() {
        let tool = FixtureTool;

        assert_eq!(
            tool.id(),
            ToolId::new("fixtures", "inspect").expect("valid id")
        );
        assert_eq!(tool.wire_name(), "inspect_wire");
        assert_eq!(tool.description(), "Inspect a fixture.");
        assert_eq!(
            tool.parameters_schema(),
            json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            })
        );
    }

    #[test]
    fn registry_lookup_uses_stable_identity_not_wire_name() {
        let tool = FixtureTool;
        let registry = ToolRegistry::new([&tool as &dyn Tool]).expect("unique registry");

        let found = registry
            .get(&ToolId::new("fixtures", "inspect").expect("valid id"))
            .expect("the stable identity should resolve");
        assert_eq!(found.wire_name(), "inspect_wire");
        assert!(
            registry
                .get(&ToolId::new("fixtures", "inspect_wire").expect("valid id"))
                .is_none(),
            "the transport name must not become identity"
        );
    }

    #[test]
    fn registry_preserves_order_and_first_match_lookup() {
        let first = RegistryFixtureTool {
            id_name: "inspect",
            wire_name: "first_inspect",
        };
        let middle = RegistryFixtureTool {
            id_name: "summarize",
            wire_name: "summarize",
        };
        let registry = ToolRegistry::new([&first as &dyn Tool, &middle as &dyn Tool])
            .expect("distinct identities build a registry");

        assert_eq!(
            registry
                .tools()
                .iter()
                .map(|tool| tool.wire_name())
                .collect::<Vec<_>>(),
            ["first_inspect", "summarize"]
        );
        assert_eq!(registry.len(), 2);
        assert_eq!(
            registry
                .get(&ToolId::new("fixtures", "inspect").expect("valid id"))
                .expect("the identity should resolve")
                .wire_name(),
            "first_inspect",
        );
    }

    #[test]
    fn registry_rejects_duplicate_tool_ids() {
        let first = RegistryFixtureTool {
            id_name: "inspect",
            wire_name: "first_inspect",
        };
        let repeated = RegistryFixtureTool {
            id_name: "inspect",
            wire_name: "second_inspect",
        };
        let error = ToolRegistry::new([&first as &dyn Tool, &repeated as &dyn Tool])
            .expect_err("a repeated tool identity must be rejected at registration");
        assert_eq!(
            error.id(),
            &ToolId::new("fixtures", "inspect").expect("valid id"),
            "the error must name the duplicated identity"
        );
    }
}
