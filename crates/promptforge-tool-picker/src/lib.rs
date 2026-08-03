//! Resolve a plain-English capability need to a tool from an abstract catalog.
//!
//! This crate is a pure, deterministic, embedding-based tool-resolution engine.
//! It takes a catalog of tool descriptors, embeds each one locally on the CPU,
//! and answers a plain-English need with one of four outcomes: a single bound
//! tool, an ambiguous duplicate pair to fail loudly on, a shortlist of
//! plausible foreign candidates, or an abstention when nothing fits.
//!
//! The engine is deliberately self-contained. It carries no Lua, no MCP or any
//! other protocol, and no network dependency: the catalog is the sole input
//! contract, the model weights are compiled into the binary, and the same
//! catalog, need, and configuration always produce the same outcome.
//!
//! Mapping a resolved descriptor onto a callable tool is the caller's job; this
//! crate only decides which descriptor the need refers to.

pub mod catalog;
pub mod config;
pub mod error;

pub use catalog::{Catalog, ToolAnnotations, ToolDescriptor, ToolId};
pub use config::{Config, ModelId};
pub use error::{Error, Result};
