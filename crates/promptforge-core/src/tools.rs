//! Tools the executor can dispatch during a model's tool-call loop.
//!
//! Some tools run locally in this process (for example fetching and rendering a
//! web page), while others proxy through the gateway so a shared credential
//! never leaves the server. Both kinds share the [`Tool`] trait so the executor
//! can dispatch them uniformly. Stable identity is separate from the wire name
//! used by the current model transport.
//!
//! This facade splits into focused child modules (tools.rs F/AUDIT-FILE-500):
//! `ids` (identity + validation errors), `output` (trusted output + the
//! model-safe error), `registry` (the [`Tool`] trait, the registry, and the
//! shared fanout set), and `web_search` (the in-crate WebSearch tool). The
//! public surface is unchanged; every public item is re-exported here.

mod ids;
mod output;
mod registry;
mod web_search;

pub use ids::{ToolId, ToolIdError, ToolIdErrorKind};
pub use output::{OutputTrust, ToolError, ToolErrorKind, ToolOutput};
pub use registry::{Tool, ToolRegistry, ToolRegistryError, ToolRegistryErrorKind};
pub use web_search::WebSearch;

pub(crate) use registry::{NearDuplicateDiagnostic, SharedTools};

#[cfg(test)]
mod tests;
