//! The tool performer: resolves a `ToolCall` effect's id in the run's
//! activated [`ToolTable`] and calls the implementation.
//!
//! The engine binds tool slots against descriptors and issues a call as a
//! [`ToolId`]; the implementations sit on the harness side, in the table
//! activation assembled for the run. An id the table does not hold is a
//! host-side fault (the engine bound a slot the catalog advertised, so the
//! table should hold it), answered as the call's own failure so the run
//! reports it at the author's call site rather than stalling.

use std::fmt;

use harness_capabilities::ToolTable;
use promptforge::tools::{ToolError, ToolErrorKind, ToolId, ToolOutput};
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
                    "tool `{alias}` ({tool}) is not among the run's activated capabilities"
                ))
                .with_kind(ToolErrorKind::Other));
            };
            implementation.call(args).await
        })
    }
}
