//! PromptForge runtime core.
//!
//! This crate holds the pieces that turn a prompt markdown file into a model
//! call: the [`parser`] that reads the file into a [`parser::Prompt`], the
//! [`client`] that talks to an `OpenAI`-compatible chat completions endpoint, and
//! [`execute`] that runs H1 once with live resolution before walking sections
//! top to bottom (fall-through) and
//! returns the run's result. [`observe`] is the seam through which a run
//! reports its progress, for a caller that wants to watch a long run in
//! flight; [`execute::run`] takes an [`execute::RunOptions`] carrying the
//! [`observe::Observer`] the borrowed correlated report records go to, and
//! [`observe::NullObserver`] is what a caller wanting silence passes.
//! [`debug::DebugCapture`] is an opt-in raw request/response seam on the same
//! options; production hosts leave it unset.
//!
//! A source is a promptforge prompt only when its frontmatter declares a
//! `promptforge:` version; [`promptforge_version`] reports it (or `None`), and
//! the runtime refuses a source that lacks a supported version.

pub mod cancel;
pub mod client;
pub mod debug;
pub mod dialects;
mod error;
pub mod execute;
pub(crate) mod fanout;
pub mod lua;
mod lua_models;
pub mod model;
pub mod normalize;
pub mod observe;
pub mod parser;
mod resolve;
pub mod store;
pub(crate) mod subst;
pub mod tools;
pub(crate) mod untrusted;

pub use crate::cancel::CancelHandle;
pub use crate::error::{Error, NearDuplicateDiagnostic, Result};
pub use crate::parser::promptforge_version;
