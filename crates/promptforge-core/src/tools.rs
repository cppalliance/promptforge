//! Tools the executor can dispatch during a model's tool-call loop.
//!
//! Some tools run locally in this process (for example fetching and rendering a
//! web page), while others proxy through the gateway so a shared credential
//! never leaves the server. Both kinds share the [`Tool`] trait so the executor
//! can dispatch them uniformly. Stable identity is separate from the wire name
//! used by the current model transport.
//!
//! The runtime-agnostic contract vocabulary ([`Tool`], [`ToolCatalog`],
//! [`ToolId`], the output and error types) lives in the `promptforge-tools`
//! crate and is re-exported here unchanged, so existing
//! `promptforge_core::tools::*` paths keep working. The concrete `WebSearch`
//! provider lives in the `promptforge-web-search` crate and is re-exported
//! here under its historical path for the same reason.

pub use promptforge_tools::{
    OutputTrust, Tool, ToolCatalog, ToolCatalogError, ToolCatalogErrorKind, ToolError,
    ToolErrorKind, ToolId, ToolIdError, ToolIdErrorKind, ToolOutput,
};
pub use promptforge_web_search::WebSearch;

/// Diagnostics for two semantic near-duplicates exposed in one model turn.
///
/// The near-duplicate check is part of tool-scope validation, so the diagnostic
/// vocabulary lives here (F10); the internal error substrate references this
/// type rather than owning it.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct NearDuplicateDiagnostic {
    /// The first prompt-local alias in scope order.
    pub(crate) first_alias: String,
    /// The first stable identity.
    pub(crate) first_id: ToolId,
    /// The second prompt-local alias in scope order.
    pub(crate) second_alias: String,
    /// The second stable identity.
    pub(crate) second_id: ToolId,
    /// The cosine similarity the picker reported at bind time.
    pub(crate) similarity: f64,
}

#[cfg(test)]
mod tests;
