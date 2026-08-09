//! Resolve a plain-English capability need to a tool from an abstract catalog.
//!
//! This crate is an embedding-based tool-resolution engine.
//! It takes a catalog of tool descriptors, embeds each one locally on the CPU,
//! and answers a plain-English need with one of four outcomes: a single bound
//! tool, a group of one server's own duplicate tools to fail loudly on, a
//! shortlist of candidates it could not separate, or an abstention when
//! nothing fits.
//!
//! The engine is deliberately self-contained. It carries no Lua, no MCP or any
//! other protocol, and no network dependency: the catalog is the sole input
//! contract, the model weights are compiled into the binary, and the same
//! catalog, need, configuration, model bytes, dependency versions, target, and
//! execution environment produce the same outcome.
//!
//! Mapping a resolved descriptor onto a callable tool is the caller's job; this
//! crate only decides which descriptor the need refers to.
//!
//! # Usage
//!
//! Build an engine over a catalog once, then ask it about as many needs as you
//! like. [`ToolPicker::resolve`] answers with a decision;
//! [`ToolPicker::shortlist`] hands back the candidates instead, for a caller
//! that would rather choose for itself.
//!
//! Loading the model is the expensive part, so a caller serving several
//! catalogs - or one catalog that changes - loads it once:
//! [`ToolPicker::build_with_model`] borrows an already-loaded [`Model`], and
//! [`ToolPicker::rebuild`] reuses this picker's model and configuration.
//!
//! ```
//! use promptforge_tool_picker::{Catalog, Config, Outcome, ToolDescriptor, ToolId, ToolPicker};
//! use serde_json::json;
//!
//! let catalog = Catalog::new(vec![
//!     ToolDescriptor::new(
//!         ToolId::new("files", "read_file"),
//!         "Read a file from disk",
//!         json!({"properties": {"path": {"type": "string"}}}),
//!     ),
//!     ToolDescriptor::new(
//!         ToolId::new("net", "fetch_url"),
//!         "Fetch a web page over HTTP",
//!         json!({"properties": {"url": {"type": "string"}}}),
//!     ),
//! ]);
//!
//! // The costly step: the model is loaded and every tool is embedded once.
//! let picker = ToolPicker::build(catalog, Config::default())?;
//!
//! match picker.resolve("read a file from disk")? {
//!     Outcome::Bind(tool) => assert_eq!(tool.name(), "read_file"),
//!     outcome => panic!("expected a binding, got {outcome:?}"),
//! }
//!
//! // No tool here writes anything, so the engine says so rather than guessing.
//! assert_eq!(picker.resolve("send an email to the team")?, Outcome::Absent);
//!
//! // A shortlist offers the same candidates, judged only against the floor.
//! let candidates = picker.shortlist("read a file from disk", 3)?;
//! assert_eq!(candidates[0].name(), "read_file");
//! # Ok::<(), promptforge_tool_picker::BuildError>(())
//! ```
//!
//! # Building this crate
//!
//! The model is compiled in from a pinned local snapshot. Set
//! `PROMPTFORGE_MODEL_DIR` to the provisioned model directory before a clean
//! build. Cargo itself performs no network access.

mod assets;
mod catalog;
mod config;
mod embed;
mod error;
mod picker;
mod policy;
mod rank;

pub use catalog::{
    Catalog, CatalogIntoIter, CatalogIter, CatalogIterMut, ToolAnnotations, ToolDescriptor, ToolId,
};
pub use config::{Config, ConfigField};
pub use embed::Model;
pub use error::{
    BuildError, ConfigError, IndexError, ModelLoadError, QueryError, QueryErrorKind, SelectionError,
};
pub use picker::{
    NearDuplicate, NearDuplicateIter, NearDuplicates, Shortlist, ToolIter, ToolPicker,
};
pub use policy::{CandidateGroup, CandidateIter, Outcome};
