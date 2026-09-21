//! The [`ToolDescriptor`]: everything an engine needs to know about a tool
//! except how to run it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ids::ToolId;
use crate::capabilities::CapabilityId;

/// One tool as data: its stable identity, transport wire name, model-facing
/// description, parameter schema, output kind, and the co-activation
/// conflicts of the capability that contributed it. Never an
/// implementation.
///
/// A host assembles descriptors from its activated capabilities into a
/// [`ToolCatalog`](super::ToolCatalog) and keeps the implementations in a
/// table of its own keyed by [`ToolId`]; the engine fills its tool slots
/// against the descriptors, advertises them, and issues each call as an
/// effect naming the id, so the host resolves the implementation and the
/// engine never holds one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolDescriptor {
    /// The stable live identity, the catalog key.
    pub id: ToolId,
    /// The transport wire name: a non-empty token without `/` or control
    /// characters, aliased when the tool is advertised to a model.
    pub wire_name: String,
    /// The one-sentence description the model reads.
    pub description: String,
    /// The JSON-Schema `object` the tool's arguments must match.
    pub parameters_schema: Value,
    /// Whether the tool's output text is one JSON value an executor resumes
    /// into the script as data rather than as a string.
    pub structured_output: bool,
    /// The capabilities the contributing capability cannot be activated
    /// with; stored for the record, checked by the host before activation.
    pub conflicts: Vec<CapabilityId>,
}

impl ToolDescriptor {
    /// Builds a plain-output descriptor with no conflicts.
    ///
    /// # Examples
    /// ```
    /// use promptforge_api_types::tools::{ToolDescriptor, ToolId};
    ///
    /// let echo = ToolDescriptor::new(
    ///     ToolId::parse("example/echo/echo")?,
    ///     "echo",
    ///     "Echo the `text` argument back.",
    ///     serde_json::json!({"type": "object", "properties": {}}),
    /// );
    /// assert_eq!(echo.wire_name, "echo");
    /// assert!(!echo.structured_output);
    /// # Ok::<(), promptforge_api_types::tools::ToolIdError>(())
    /// ```
    #[must_use]
    pub fn new(
        id: ToolId,
        wire_name: impl Into<String>,
        description: impl Into<String>,
        parameters_schema: Value,
    ) -> ToolDescriptor {
        ToolDescriptor {
            id,
            wire_name: wire_name.into(),
            description: description.into(),
            parameters_schema,
            structured_output: false,
            conflicts: Vec::new(),
        }
    }

    /// Marks the descriptor's output as structured JSON (or plain text).
    #[must_use]
    pub fn structured(mut self, structured: bool) -> ToolDescriptor {
        self.structured_output = structured;
        self
    }

    /// Records the contributing capability's co-activation conflicts.
    #[must_use]
    pub fn with_conflicts(mut self, conflicts: Vec<CapabilityId>) -> ToolDescriptor {
        self.conflicts = conflicts;
        self
    }
}
