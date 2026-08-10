//! Resolve a plain-English capability need to a tool from an abstract catalog.
//!
//! This crate is a pure, deterministic, embedding-based tool-resolution engine.
//! It takes a [`Catalog`] of [`ToolDescriptor`] values, embeds each one locally
//! on the CPU with a reusable [`Model`], and answers a need with one of four
//! borrowing outcomes: a single bound tool, a group of one server's own
//! duplicate tools to fail loudly on, a shortlist of candidates it could not
//! separate, or an abstention when nothing fits.
//!
//! Mapping a resolved descriptor onto a callable tool is the caller's job; this
//! crate only decides which descriptor the need refers to.
//!
//! # Usage
//!
//! Build a picker over a catalog once with [`ToolPicker::build`], then ask it
//! about as many needs as you like. [`ToolPicker::resolve`] answers with a
//! decision; [`ToolPicker::shortlist`] hands back candidates for a caller that
//! would rather choose for itself. A caller serving several catalogs loads a
//! [`Model`] once and reuses it through [`ToolPicker::build_with_model`] or
//! [`ToolPicker::rebuild`].
//!
//! Query results borrow the picker, so no descriptor or schema is deep-cloned;
//! clone the specific [`ToolId`] or descriptor you need to keep.
//!
//! ```no_run
//! use promptforge_tool_picker::{Catalog, Config, Outcome, ToolDescriptor, ToolId, ToolPicker};
//! use serde_json::json;
//!
//! let catalog = Catalog::new(vec![ToolDescriptor::new(
//!     ToolId::new("files", "read_file"),
//!     "Read a file from disk",
//!     json!({"properties": {"path": {"type": "string"}}}),
//! )]);
//! let picker = ToolPicker::build(catalog, Config::default())?;
//! match picker.resolve("read a file from disk")? {
//!     Outcome::Bind(tool) => assert_eq!(tool.name(), "read_file"),
//!     _ => {}
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Determinism
//!
//! The same model bytes, dependency versions, target, execution environment,
//! catalog, configuration, and need always produce the same outcome.
//! Cross-platform byte-identical vectors at floating-point boundaries are not
//! promised.

mod assets;
mod catalog;
mod config;
mod embed;
mod error;
mod model;
mod picker;
mod policy;
mod rank;
mod selected;

pub use catalog::{
    Catalog, CatalogIntoIter, CatalogIter, CatalogIterMut, ToolAnnotations, ToolDescriptor, ToolId,
};
pub use config::{Config, ConfigError, ConfigField};
pub use error::{
    BuildError, IndexError, ModelLoadError, QueryError, QueryErrorKind, SelectionError,
};
pub use model::Model;
pub use picker::{ToolIter, ToolPicker};
pub use policy::{CandidateGroup, CandidateIter, Outcome, Shortlist};
pub use selected::{NearDuplicate, NearDuplicateIter, NearDuplicates};
