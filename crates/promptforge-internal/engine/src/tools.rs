//! The tool vocabulary the executor binds and advertises.
//!
//! The Engine fills its tool slots by identity against the caller-supplied
//! [`ToolCatalog`] of descriptors and issues each call as a `ToolCall`
//! effect naming the [`ToolId`] for the caller to perform. The
//! runtime-agnostic vocabulary - [`ToolCatalog`], [`ToolId`], the output
//! and error types - sits in the `promptforge-types` crate's `tools`
//! module; this module is the
//! crate-internal import surface for it, and other crates name the
//! vocabulary through `promptforge_types::tools`.

#[cfg(test)]
pub(crate) use promptforge_types::tools::{OutputTrust, ToolError, ToolErrorKind, ToolOutput};
pub(crate) use promptforge_types::tools::{ToolCatalog, ToolId};

#[cfg(test)]
#[path = "tools-tests.rs"]
mod tests;
