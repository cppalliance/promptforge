//! Who made one tool call: [`ToolCallOrigin`] and its [`ToolCaller`].

use serde::{Deserialize, Serialize};

/// Records who made one tool call and where.
///
/// It names the run's execution, the section whose Lua was running, and
/// whether the section's script or a model round asked for the call. A log
/// uses it to attribute the call. The application can use it to
/// apply different policy to the same tool depending on who called it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolCallOrigin {
    /// The run's execution identifier.
    pub execution: String,
    /// The name of the section that made the call.
    pub section: String,
    /// Which kind of code asked for the call.
    pub caller: ToolCaller,
}

impl ToolCallOrigin {
    /// Builds the origin of a call `caller` made from `section` in the run
    /// `execution`.
    #[must_use]
    pub fn new(
        execution: impl Into<String>,
        section: impl Into<String>,
        caller: ToolCaller,
    ) -> ToolCallOrigin {
        ToolCallOrigin {
            execution: execution.into(),
            section: section.into(),
            caller,
        }
    }
}

/// Which kind of code asked for one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolCaller {
    /// The section's own Lua called the tool through `tools.call`.
    Script,
    /// A model round requested the call.
    Model,
}
