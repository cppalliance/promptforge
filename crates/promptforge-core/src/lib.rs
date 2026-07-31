//! PromptForge runtime core.
//!
//! This crate holds the pieces that turn a prompt markdown file into a model
//! call: the [`parser`] that reads the file into a [`parser::Prompt`], the
//! [`client`] that talks to an `OpenAI`-compatible chat completions endpoint, and
//! [`execute`] that walks a prompt's sections top to bottom (fall-through) and
//! returns the run's result.
//!
//! A source is a promptforge prompt only when its frontmatter declares a
//! `promptforge:` version; [`promptforge_version`] reports it (or `None`), and
//! the runtime refuses a source that lacks a supported version.

pub mod client;
mod error;
pub mod execute;
pub mod lua;
pub mod parser;
pub mod store;
pub mod subst;
pub mod tools;

pub use crate::error::{Error, Result};
pub use crate::parser::promptforge_version;
