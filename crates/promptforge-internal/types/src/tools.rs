//! The runtime-agnostic PromptForge tool vocabulary.
//!
//! Performing a tool call is the Harness's job. The Engine binds and
//! advertises tools as data and issues each call as an effect naming the
//! tool's stable identity ([`ToolId`]), which is separate from the wire name
//! used by the current model transport.
//!
//! This module holds vocabulary only: the implementation-free
//! [`ToolDescriptor`] and the caller-supplied [`ToolCatalog`] of descriptors,
//! trusted output ([`ToolOutput`], [`OutputTrust`]), the model-safe
//! [`ToolError`], and the contract errors. The tool implementation behind a
//! descriptor is the Harness's; the prompt parser and the executor sit in
//! their own crates and depend on `promptforge-types`.

mod descriptor;
mod ids;
mod output;
mod registry;

pub use descriptor::ToolDescriptor;
pub use ids::{ToolId, ToolIdError, ToolIdErrorKind};
pub use output::{OutputTrust, ToolError, ToolErrorKind, ToolOutput};
pub use registry::{ToolCatalog, ToolCatalogError, ToolCatalogErrorKind};

#[cfg(test)]
mod tests;
