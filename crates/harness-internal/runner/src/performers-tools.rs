//! The tool performer: resolves a `ToolCall` effect's id in the run's
//! activated [`ToolTable`] and calls the implementation, lending it the
//! effect's access and origin through a [`ToolContext`].
//!
//! The Engine binds tool slots against descriptors and issues a call as a
//! [`ToolId`]; the implementations sit on the Harness side, in the table
//! activation assembled for the run. An id the table does not hold is a
//! Harness fault (the Engine bound a slot the catalog advertised, so the
//! table should hold it), answered as the call's own failure so the run
//! reports it at the author's call site rather than stalling.

use std::fmt;
use std::sync::Arc;

use harness_plugins::{ToolContext, ToolTable};
use promptforge::effect::ToolCallOrigin;
use promptforge::tools::{ToolError, ToolErrorKind, ToolId, ToolOutput};
use promptforge::vfs::Access;
use serde_json::Value;

use super::{BoxFuture, ToolPerformer};

/// Performs `ToolCall` effects against the run's activated tools.
#[derive(Clone)]
pub struct ActivatedTools {
    table: ToolTable,
}

impl ActivatedTools {
    /// A performer over `table`: the implementations behind the catalog
    /// the run was prepared with.
    #[must_use]
    pub fn new(table: ToolTable) -> Self {
        Self { table }
    }
}

impl fmt::Debug for ActivatedTools {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActivatedTools")
            .field("table", &self.table)
            .finish()
    }
}

impl ToolPerformer for ActivatedTools {
    fn call(
        &self,
        tool: ToolId,
        alias: String,
        access: Arc<Access>,
        origin: ToolCallOrigin,
        args: Value,
    ) -> BoxFuture<Result<ToolOutput, ToolError>> {
        let resolved = self.table.get(&tool);
        Box::pin(async move {
            let Some(implementation) = resolved else {
                tracing::error!(
                    tool = %tool,
                    alias = %alias,
                    "a ToolCall names an id outside the run's activated table"
                );
                return Err(ToolError::message(format!(
                    "tool `{alias}` ({tool}) is not among the run's activated Plugins"
                ))
                .with_kind(ToolErrorKind::Other));
            };
            implementation
                .call(ToolContext::new(&access, &origin), args)
                .await
        })
    }

    fn survives_stop(&self, tool: &ToolId) -> bool {
        self.table
            .get(tool)
            .is_some_and(|implementation| implementation.descriptor().survives_stop)
    }
}
