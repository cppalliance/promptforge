//! The [`Tool`] trait, the tool registry and its errors, and the shared
//! tool set used by concurrent fanout arms.

use std::sync::Arc;

use super::ids::{ToolId, validate_identifier};
use super::output::{ToolError, ToolOutput};

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
            // `Arc::<[T]>::from(&[T])` clones each element straight into the
            // ref-counted slice; no intermediate owned `Vec` is allocated first.
            tools: Arc::from(tools),
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

/// Diagnostics for two semantic near-duplicates exposed in one model turn.
///
/// The near-duplicate check is part of tool-scope validation, so the diagnostic
/// vocabulary lives here (F10); the internal error substrate references this
/// type rather than owning it.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct NearDuplicateDiagnostic {
    /// The first prompt-local alias in picker catalog pair order.
    pub(crate) first_alias: String,
    /// The first stable identity.
    pub(crate) first_id: ToolId,
    /// The second prompt-local alias in picker catalog pair order.
    pub(crate) second_alias: String,
    /// The second stable identity.
    pub(crate) second_id: ToolId,
    /// The cosine similarity reported by the picker.
    pub(crate) similarity: f32,
}
/// A stable, matchable classification of a [`ToolRegistryError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolRegistryErrorKind {
    /// Two supplied tools shared a stable [`ToolId`].
    DuplicateId,
    /// A supplied tool's [`wire_name`](Tool::wire_name) was not transport-legal.
    InvalidWireName,
}

/// A [`ToolRegistry`] could not be built from the supplied tools.
///
/// This classifying error supersedes the design's `DuplicateToolId` name
/// (DESIGN-2.4): the registry is the schema/transport boundary, so besides
/// rejecting a repeated identity it also rejects a tool whose
/// [`wire_name`](Tool::wire_name) is empty or carries a separator or control
/// character (tools.rs F4). It exposes a stable [`kind`](Self::kind) classifier
/// (DESIGN-5).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ToolRegistryError {
    /// The same stable identity was supplied by more than one tool.
    #[error("duplicate tool identity {id:?} in registry")]
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

impl ToolRegistryError {
    /// Returns the stable classification of this error (DESIGN-5).
    #[must_use]
    pub fn kind(&self) -> ToolRegistryErrorKind {
        match self {
            ToolRegistryError::DuplicateId { .. } => ToolRegistryErrorKind::DuplicateId,
            ToolRegistryError::InvalidWireName { .. } => ToolRegistryErrorKind::InvalidWireName,
        }
    }

    /// Returns the duplicated identity when this is a [`Self::DuplicateId`].
    #[must_use]
    pub fn duplicate_id(&self) -> Option<&ToolId> {
        match self {
            ToolRegistryError::DuplicateId { id } => Some(id),
            ToolRegistryError::InvalidWireName { .. } => None,
        }
    }
}

impl From<ToolRegistryError> for crate::error::Error {
    fn from(error: ToolRegistryError) -> Self {
        match error {
            ToolRegistryError::DuplicateId { id } => {
                crate::error::Error::DuplicateLiveToolId { id }
            }
            // Preserve the structured registry error (rejected name + reason)
            // as a private `#[source]` cause instead of flattening it to a bare
            // reason string (AUDIT-DISCARDED-SOURCE).
            invalid @ ToolRegistryError::InvalidWireName { .. } => {
                crate::error::Error::InvalidToolWireName {
                    source: Box::new(invalid),
                }
            }
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
/// use promptforge_core::tools::{
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
/// let echo = Echo { id: ToolId::new("example", "echo")? };
/// assert_eq!(echo.wire_name(), "echo");
/// assert_eq!(echo.id().server(), "example");
/// # let _ = OutputTrust::Trusted;
/// # Ok::<(), promptforge_core::tools::ToolIdError>(())
/// ```
///
/// # Compatibility policy
///
/// This trait is a stable extension point and is deliberately open. Adding a
/// **new required** method (one without a default body) is a breaking change for
/// downstream implementers; new capabilities must therefore ship with a default
/// implementation. Existing method signatures are stable. `ToolId`,
/// `ToolRegistry`, `ToolError`, and `ToolOutput` are `#[non_exhaustive]` so they
/// can gain fields or variants without a break.
///
/// # Invariants
///
/// - [`id`](Tool::id) returns the same value on every call for a given tool; it
///   is the registry key and must be unique within a [`ToolRegistry`].
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
    /// This is the registry key. It must be stable across calls and unique
    /// within any [`ToolRegistry`] the tool is registered in.
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

/// An ordered collection of callable live tools with unique identities.
///
/// Registration rejects repeated stable identities: a [`ToolRegistry`] can never
/// hold two tools that share a [`ToolId`]. Iteration preserves supplied order and
/// lookup is identity-based.
#[non_exhaustive]
pub struct ToolRegistry<'a> {
    /// The live tools in supplied order; [`ToolRegistry::tools`] borrows this.
    tools: Vec<&'a dyn Tool>,
    /// Each tool's validated stable identity, parallel to `tools` by index.
    ///
    /// Caching identities here means [`ToolRegistry::get`] compares against a
    /// stored [`ToolId`] instead of calling [`Tool::id`] (which allocates two
    /// `String`s) for every entry on every lookup (tools.rs F9).
    ids: Vec<ToolId>,
}

impl std::fmt::Debug for ToolRegistry<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolRegistry")
            .field("ids", &self.ids)
            .finish()
    }
}

impl<'a> ToolRegistry<'a> {
    /// Builds a registry in the order the live tools are supplied.
    ///
    /// # Errors
    /// Returns [`ToolRegistryError::DuplicateId`] if two supplied tools share a
    /// [`ToolId`] (the registry never holds duplicate identities), or
    /// [`ToolRegistryError::InvalidWireName`] if a tool's
    /// [`wire_name`](Tool::wire_name) is empty or carries a `/` separator or a
    /// control character.
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
        let mut ids = Vec::with_capacity(tools.len());
        let mut seen = std::collections::BTreeSet::new();
        for tool in &tools {
            // The registry is the transport boundary: reject a wire name that is
            // empty or carries a separator/control character (tools.rs F4).
            if let Err(error) = validate_identifier("wire name", tool.wire_name()) {
                return Err(ToolRegistryError::InvalidWireName {
                    wire_name: tool.wire_name().to_owned(),
                    reason: error.reason(),
                });
            }
            let id = tool.id();
            if !seen.insert(id.clone()) {
                return Err(ToolRegistryError::DuplicateId { id });
            }
            ids.push(id);
        }
        Ok(Self { tools, ids })
    }

    /// Builds a registry from tools whose identities are already known unique.
    ///
    /// For internal callers that validated uniqueness when the tool set was
    /// assembled (see [`SharedTools`]), avoiding a redundant second scan.
    pub(crate) fn from_unique(tools: impl IntoIterator<Item = &'a dyn Tool>) -> ToolRegistry<'a> {
        let tools: Vec<&'a dyn Tool> = tools.into_iter().collect();
        let ids = tools.iter().map(|tool| tool.id()).collect();
        Self { tools, ids }
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
    /// let missing = ToolId::new("promptforge", "missing")?;
    /// assert!(registry.get(&missing).is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&'a dyn Tool> {
        // Compare against the cached identity so a lookup never re-derives every
        // entry's `ToolId` (two `String` allocations each) via `Tool::id`.
        self.ids
            .iter()
            .position(|entry| entry == id)
            .map(|index| self.tools[index])
    }
}
