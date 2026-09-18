//! Tools the executor can dispatch during a model's tool-call loop.
//!
//! Some tools run locally in this process (for example fetching and rendering a
//! web page), while others proxy through the gateway so a shared credential
//! never leaves the server. Both kinds share one `Tool` trait so the executor
//! can dispatch them uniformly. Stable identity is separate from the wire name
//! used by the current model transport.
//!
//! The runtime-agnostic contract vocabulary (the `Tool` trait,
//! [`ToolCatalog`], [`ToolId`], the output and error types) lives in the
//! `promptforge-api-types` crate's `tools` module, and the concrete
//! `WebSearch` provider lives in the `promptforge-web-search` crate. This
//! module is the crate-internal import surface for both; hosts name the
//! contract through `promptforge_api_types::tools`.

#[cfg(test)]
pub(crate) use promptforge_api_types::tools::{
    OutputTrust, Tool, ToolError, ToolErrorKind, ToolOutput,
};
pub(crate) use promptforge_api_types::tools::{ToolCatalog, ToolId};
#[cfg(test)]
pub(crate) use promptforge_web_search::WebSearch;

#[cfg(test)]
#[path = "tools-tests.rs"]
mod tests;
