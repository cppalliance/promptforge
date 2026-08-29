//! The runtime-agnostic PromptForge tool contract.
//!
//! Some tools run locally in the caller's process (for example fetching and
//! rendering a web page), while others proxy through a gateway so a shared
//! credential never leaves the server. Both kinds share the [`Tool`] trait so
//! an executor can dispatch them uniformly. Stable identity ([`ToolId`]) is
//! separate from the wire name used by the current model transport.
//!
//! This crate holds vocabulary only: the [`Tool`] trait, the caller-provided
//! [`ToolCatalog`], trusted output ([`ToolOutput`], [`OutputTrust`]), the
//! model-safe [`ToolError`], and the contract errors. Concrete tool
//! implementations, the prompt parser, and the executor live in their own
//! crates and depend on this one.

mod ids;
mod output;
mod registry;

pub use ids::{ToolId, ToolIdError, ToolIdErrorKind};
pub use output::{OutputTrust, ToolError, ToolErrorKind, ToolOutput};
pub use registry::{Tool, ToolCatalog, ToolCatalogError, ToolCatalogErrorKind};

#[cfg(test)]
mod tests;
