//! The tool vocabulary the executor offers and advertises.
//!
//! The caller-supplied [`ToolCatalog`] of descriptors lists every tool the
//! run can use: the prompt's Lua calls one by id, and a section offers one
//! to its model with `tools.offer`. The Engine issues each call as a
//! `ToolCall` effect naming the [`ToolId`](promptforge_types::tools::ToolId)
//! for the caller to perform; the installed Plugin the id names answers it
//! (the `Plugin` trait, in `promptforge-plugin`). The runtime-agnostic
//! vocabulary - [`ToolCatalog`], [`ToolId`](promptforge_types::tools::ToolId),
//! the output and error types - sits in the `promptforge-types` crate's
//! `tools` module; this module is the crate-internal import surface for it,
//! and other crates name the vocabulary through `promptforge_types::tools`.

pub(crate) use promptforge_types::tools::ToolCatalog;
