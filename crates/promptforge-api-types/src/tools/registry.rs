//! The [`Tool`] trait, the host-supplied [`ToolCatalog`] of tool
//! descriptors, and the catalog's construction error.

use std::sync::Arc;

use super::descriptor::ToolDescriptor;
use super::ids::{ToolId, validate_identifier};
use super::output::{ToolError, ToolOutput};

/// The host-supplied catalog of the tools a run may bind, as descriptors.
///
/// The host assembles the catalog from its activated capabilities and keeps
/// the implementations in a table of its own: the engine fills its tool
/// slots against the descriptors and never holds an implementation.
/// Construction rejects a repeated [`ToolId`] or a transport-illegal wire
/// name, so the bind-phase [`get`](Self::get) lookup trusts the invariant
/// without rescanning. Cloning is cheap: the descriptors live behind one
/// refcounted slice.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct ToolCatalog {
    tools: Arc<[ToolDescriptor]>,
}

impl std::fmt::Debug for ToolCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolCatalog")
            .field(
                "ids",
                &self.tools.iter().map(|tool| &tool.id).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ToolCatalog {
    /// Builds a catalog from tool descriptors.
    ///
    /// Validates identity uniqueness and wire-name legality once, here, so
    /// [`Self::get`] can trust the invariant without rescanning.
    ///
    /// # Errors
    /// Returns [`ToolCatalogError::DuplicateId`] if two descriptors share a
    /// [`ToolId`], or [`ToolCatalogError::InvalidWireName`] if a
    /// descriptor's wire name is empty or carries a `/` separator or a
    /// control character.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_api_types::tools::ToolCatalog;
    ///
    /// let catalog = ToolCatalog::new(&[])?;
    /// assert!(catalog.tools().is_empty());
    /// # Ok::<(), promptforge_api_types::tools::ToolCatalogError>(())
    /// ```
    pub fn new(tools: &[ToolDescriptor]) -> Result<Self, ToolCatalogError> {
        let mut seen = std::collections::BTreeSet::new();
        for tool in tools {
            // The catalog is the transport boundary: reject a wire name that
            // is empty or carries a separator/control character (tools.rs F4).
            if let Err(error) = validate_identifier("wire name", &tool.wire_name) {
                return Err(ToolCatalogError::InvalidWireName {
                    wire_name: tool.wire_name.clone(),
                    reason: error.reason(),
                });
            }
            if !seen.insert(tool.id.clone()) {
                return Err(ToolCatalogError::DuplicateId {
                    id: tool.id.clone(),
                });
            }
        }
        Ok(Self {
            // `Arc::<[T]>::from(&[T])` clones each element straight into the
            // ref-counted slice; no intermediate owned `Vec` is allocated first.
            tools: Arc::from(tools),
        })
    }

    /// Returns the descriptor for `id`, if one is in the catalog.
    ///
    /// This is the bind-time lookup, a cold path run once per declared
    /// slot, so it scans linearly rather than carrying a cached-identity
    /// index.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_api_types::tools::{ToolCatalog, ToolId};
    ///
    /// let catalog = ToolCatalog::new(&[])?;
    /// let missing = ToolId::parse("promptforge/tools/missing")?;
    /// assert!(catalog.get(&missing).is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&ToolDescriptor> {
        self.tools.iter().find(|tool| tool.id == *id)
    }

    /// Returns the catalog's descriptors in supplied order.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_api_types::tools::ToolCatalog;
    ///
    /// let catalog = ToolCatalog::new(&[])?;
    /// assert!(catalog.tools().is_empty());
    /// # Ok::<(), promptforge_api_types::tools::ToolCatalogError>(())
    /// ```
    #[must_use]
    pub fn tools(&self) -> &[ToolDescriptor] {
        &self.tools
    }
}

impl ToolDescriptor {
    /// Describes an implementation: the descriptor a host derives from a
    /// [`Tool`] when it assembles its catalog, with no conflicts recorded
    /// (the contributing capability's are added by the host).
    ///
    /// # Examples
    /// ```
    /// use promptforge_api_types::tools::{Tool, ToolDescriptor, ToolError, ToolId, ToolOutput};
    ///
    /// struct Echo;
    ///
    /// #[async_trait::async_trait]
    /// impl Tool for Echo {
    ///     fn id(&self) -> ToolId {
    ///         ToolId::parse("example/echo/echo").expect("the id is valid")
    ///     }
    ///     fn wire_name(&self) -> &str {
    ///         "echo"
    ///     }
    ///     fn description(&self) -> &str {
    ///         "Echo the `text` argument back."
    ///     }
    ///     fn parameters_schema(&self) -> serde_json::Value {
    ///         serde_json::json!({"type": "object", "properties": {}})
    ///     }
    ///     async fn call(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError> {
    ///         Ok(ToolOutput::trusted(args.to_string()))
    ///     }
    /// }
    ///
    /// let descriptor = ToolDescriptor::describe(&Echo);
    /// assert_eq!(descriptor.wire_name, "echo");
    /// assert_eq!(descriptor.id.name(), "echo");
    /// ```
    #[must_use]
    pub fn describe(tool: &dyn Tool) -> ToolDescriptor {
        ToolDescriptor::new(
            tool.id(),
            tool.wire_name(),
            tool.description(),
            tool.parameters_schema(),
        )
        .structured(tool.structured_output())
    }
}

/// Describes every implementation in `tools`, in order.
///
/// A convenience over [`ToolDescriptor::describe`] for a host assembling a
/// catalog from a contribution's tool arcs.
#[must_use]
pub fn describe_all(tools: &[Arc<dyn Tool>]) -> Vec<ToolDescriptor> {
    tools
        .iter()
        .map(|tool| ToolDescriptor::describe(tool.as_ref()))
        .collect()
}

/// A stable, matchable classification of a [`ToolCatalogError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolCatalogErrorKind {
    /// Two supplied tools shared a stable [`ToolId`].
    DuplicateId,
    /// A supplied tool's [`wire_name`](Tool::wire_name) was not transport-legal.
    InvalidWireName,
}

/// A [`ToolCatalog`] could not be built from the supplied tools.
///
/// This classifying error supersedes the design's `DuplicateToolId` name
/// (DESIGN-2.4): the catalog is the schema/transport boundary, so besides
/// rejecting a repeated identity it also rejects a tool whose
/// [`wire_name`](Tool::wire_name) is empty or carries a separator or control
/// character (tools.rs F4). It exposes a stable [`kind`](Self::kind) classifier
/// (DESIGN-5).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ToolCatalogError {
    /// The same stable identity was supplied by more than one tool.
    #[error("duplicate tool identity {id:?} in the tool catalog")]
    #[non_exhaustive]
    DuplicateId {
        /// The stable identity supplied more than once.
        id: ToolId,
    },
    /// A tool's transport wire name was not a legal identifier.
    #[error("invalid tool wire name {wire_name:?}: {reason}")]
    #[non_exhaustive]
    InvalidWireName {
        /// The rejected wire name.
        wire_name: String,
        /// Why it was rejected.
        reason: &'static str,
    },
}

impl ToolCatalogError {
    /// Returns the stable classification of this error (DESIGN-5).
    #[must_use]
    pub fn kind(&self) -> ToolCatalogErrorKind {
        match self {
            ToolCatalogError::DuplicateId { .. } => ToolCatalogErrorKind::DuplicateId,
            ToolCatalogError::InvalidWireName { .. } => ToolCatalogErrorKind::InvalidWireName,
        }
    }

    /// Returns the duplicated identity when this is a [`Self::DuplicateId`].
    #[must_use]
    pub fn duplicate_id(&self) -> Option<&ToolId> {
        match self {
            ToolCatalogError::DuplicateId { id } => Some(id),
            ToolCatalogError::InvalidWireName { .. } => None,
        }
    }
}

/// A tool the executor can dispatch during a model's tool-call loop.
///
/// # Implementing
///
/// A complete implementation supplies a stable identity, a transport wire name,
/// a model-facing description, a JSON-Schema parameter object, and an async
/// [`call`](Tool::call). A minimal doctested implementation:
///
/// ```
/// use promptforge_api_types::tools::{
///     OutputTrust, Tool, ToolError, ToolErrorKind, ToolId, ToolOutput,
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
/// # let _ = OutputTrust::Trusted;
/// # Ok::<(), promptforge_api_types::tools::ToolIdError>(())
/// ```
///
/// # Compatibility policy
///
/// This trait is a stable extension point and is deliberately open. Adding a
/// **new required** method (one without a default body) is a breaking change for
/// downstream implementers; new capabilities must therefore ship with a default
/// implementation. Existing method signatures are stable. `ToolId`,
/// `ToolCatalog`, `ToolError`, and `ToolOutput` are `#[non_exhaustive]` so they
/// can gain fields or variants without a break.
///
/// # Invariants
///
/// - [`id`](Tool::id) returns the same value on every call for a given tool; it
///   is the catalog key and must be unique within a [`ToolCatalog`] (whose
///   entries are the tool's [`ToolDescriptor`]).
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
    /// within any [`ToolCatalog`] the tool is described into.
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
    /// fails rather than smuggling attacker-shaped data past the guard.
    fn structured_output(&self) -> bool {
        false
    }

    /// Execute the tool with the given JSON arguments and return its output.
    ///
    /// The returned [`ToolOutput`] carries its own
    /// [`OutputTrust`](crate::tools::OutputTrust), so trust is mandatory and
    /// cannot be forgotten: an
    /// [`OutputTrust::Untrusted`](crate::tools::OutputTrust::Untrusted) result
    /// is nonce-wrapped before it can reach model input. A failure returns a
    /// narrow, model-safe [`ToolError`]. Implementations must not panic and
    /// should return promptly when the run is cancelled.
    ///
    /// # Errors
    /// Returns a [`ToolError`] if the arguments are unacceptable, the backend
    /// refuses, the transport fails, or the run is cancelled.
    async fn call(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError>;
}
