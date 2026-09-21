//! The tool vocabulary the executor binds and advertises.
//!
//! The engine fills its tool slots by identity against the host-supplied
//! [`ToolCatalog`] of descriptors and issues each call as a `ToolCall`
//! effect naming the [`ToolId`], which the host resolves against its own
//! implementations (the harness's `Tool` trait, in
//! `harness-capabilities`). The runtime-agnostic vocabulary -
//! [`ToolCatalog`], [`ToolId`], the output and error types - sits in the
//! `promptforge-api-types` crate's `tools` module; this module is the
//! crate-internal import surface for it, and hosts name the vocabulary
//! through `promptforge_api_types::tools`.

#[cfg(test)]
pub(crate) use promptforge_api_types::tools::{OutputTrust, ToolError, ToolErrorKind, ToolOutput};
pub(crate) use promptforge_api_types::tools::{ToolCatalog, ToolId};

#[cfg(test)]
#[path = "tools-tests.rs"]
mod tests;
