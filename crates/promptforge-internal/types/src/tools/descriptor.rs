//! The [`ToolDescriptor`]: everything an Engine needs to know about a tool
//! except how to run it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ids::ToolId;
use crate::capabilities::CapabilityId;

/// A tool described as data.
///
/// A descriptor holds the tool's stable id, its wire name, the description
/// the model reads, its parameter schema, whether its output is structured,
/// and the capabilities that conflict with the contributing capability.
///
/// The caller collects the descriptors of its activated capabilities into a
/// [`ToolCatalog`](super::ToolCatalog). The implementations stay in the
/// caller's own table, keyed by [`ToolId`]. The Engine binds its tool slots
/// to the descriptors and advertises them to the model. It issues each tool
/// call as an effect that names the tool's id. The caller resolves that id
/// to the implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolDescriptor {
    /// The tool's stable identity. The catalog uses it as the key.
    pub id: ToolId,
    /// The name the tool is sent under on the wire. It must be non-empty and
    /// free of `/` and control characters. When the tool is advertised to a
    /// model, it appears under its prompt-local alias.
    pub wire_name: String,
    /// The one-sentence description the model reads.
    pub description: String,
    /// The JSON-Schema `object` the tool's arguments must match.
    pub parameters_schema: Value,
    /// Whether the tool's output text is a single JSON value. When it is, a
    /// script that calls the tool receives the output as data. Otherwise the
    /// script receives it as a string.
    pub structured_output: bool,
    /// The capabilities that conflict with the contributing capability. Each
    /// of them and the contributing capability exclude each other in a run.
    /// The descriptor only records them. The caller checks them before
    /// activation.
    pub conflicts: Vec<CapabilityId>,
}

impl ToolDescriptor {
    /// Builds a descriptor with plain-text output and an empty conflict list.
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

    /// Sets whether the tool's output is structured JSON (`true`) or plain
    /// text (`false`).
    #[must_use]
    pub fn structured(mut self, structured: bool) -> ToolDescriptor {
        self.structured_output = structured;
        self
    }

    /// Sets the capabilities that conflict with the contributing capability.
    /// Each of them and the contributing capability exclude each other in a
    /// run.
    #[must_use]
    pub fn with_conflicts(mut self, conflicts: Vec<CapabilityId>) -> ToolDescriptor {
        self.conflicts = conflicts;
        self
    }
}
