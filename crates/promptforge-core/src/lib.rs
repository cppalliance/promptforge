//! PromptForge runtime core.
//!
//! This crate holds the pieces that turn a prompt markdown file into a model
//! call: the [`parser`] that reads the file into a [`parser::Prompt`], the
//! [`client`] that talks to an `OpenAI`-compatible chat completions endpoint, and
//! [`execute`] that runs H1 once with live resolution before walking sections
//! top to bottom (fall-through) and
//! returns the run's result. [`observe`] is the seam through which a run
//! reports its progress, for a caller that wants to watch a long run in
//! flight; [`execute::run`] takes an [`execute::RunConfig`] carrying the
//! [`observe::Observer`] the correlated report records go to, and
//! [`observe::NullObserver`] is what a caller wanting silence passes.
//! [`debug::DebugCapture`] is an opt-in raw request/response seam on the same
//! config; production hosts leave it unset.
//!
//! A source is a promptforge prompt only when its frontmatter declares a
//! `promptforge:` version; [`promptforge_version`] reports it (or `None`), and
//! the runtime refuses a source that lacks a supported version.
//!
//! # Examples
//!
//! Detect a promptforge source and parse it into a [`Prompt`]:
//!
//! ```
//! use promptforge_core::{Prompt, promptforge_version};
//! use promptforge_core::observe::NullObserver;
//!
//! let source = "---\nname: greeter\ndescription: says hi\npromptforge: 1\n---\n\n# Greeter\n\n## Say hi\n\nSay hello.\n";
//!
//! // Version detection gates whether the runtime will accept the source.
//! assert_eq!(promptforge_version(source), Some(1));
//! assert_eq!(promptforge_version("plain text, no frontmatter"), None);
//!
//! let prompt = Prompt::parse(source, "doc-example", &NullObserver::default())?;
//! assert_eq!(prompt.title(), "Greeter");
//! assert_eq!(prompt.sections()[0].name(), "Say hi");
//! # Ok::<(), promptforge_core::ParseError>(())
//! ```
//!
//! Executing a parsed prompt goes through [`run`] with a [`RunConfig`], a
//! [`ResolutionContext`] (picker, model catalog, and tool catalog), and a
//! store; that path can perform gateway I/O, so it is shown as `no_run`:
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use promptforge_core::{Prompt, ResolutionContext, RunConfig, run};
//! use promptforge_core::model::ModelCatalog;
//! use promptforge_core::observe::NullObserver;
//! use promptforge_core::store::StoreRef;
//! use promptforge_core::tools::ToolCatalog;
//! use promptforge_tool_picker::{Catalog, Config, ToolPicker};
//!
//! let source = "---\nname: greeter\ndescription: says hi\npromptforge: 1\n---\n\n# Greeter\n\n## Say hi\n\nSay hello.\n";
//! let prompt = Prompt::parse(source, "run-example", &NullObserver::default())?;
//!
//! let picker = ToolPicker::build(Catalog::new(Vec::new()), Config::default())?;
//! let models = ModelCatalog::empty();
//! let tools = ToolCatalog::new(&[])?;
//! let answer = run(
//!     &prompt,
//!     "",
//!     ResolutionContext::new(&picker, &models, &tools),
//!     &StoreRef::memory(),
//!     RunConfig::new("run-example"),
//! )
//! .await?;
//! println!("{answer}");
//! # Ok(())
//! # }
//! ```

pub(crate) mod cancel;
pub mod client;
pub mod debug;
pub mod dialects;
mod error;
pub mod execute;
pub(crate) mod fanout;
pub(crate) mod lua;
mod lua_models;
pub mod model;
pub(crate) mod normalize;
pub mod observe;
pub mod parser;
mod resolve;
pub mod store;
pub(crate) mod subst;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tools;
pub(crate) mod untrusted;

pub(crate) use crate::error::{Error, Result};
pub(crate) use crate::tools::NearDuplicateDiagnostic;

pub use crate::cancel::CancelHandle;
pub use crate::dialects::{DialectError, DialectErrorKind};
pub use crate::execute::{ResolutionContext, RunConfig, RunError, RunErrorKind, RunLimits, run};
pub use crate::model::{CompletionError, CompletionErrorKind};
pub use crate::parser::{ParseError, ParseErrorKind, Prompt, promptforge_version};
