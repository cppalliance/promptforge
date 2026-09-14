//! PromptForge runtime core.
//!
//! This crate holds the pieces that turn a prompt markdown file into a model
//! call: the [`parser`] that reads the file into a [`parser::Prompt`], the
//! [`client`] that talks to an `OpenAI`-compatible chat completions endpoint, and
//! [`execute`] that runs H1 once with live resolution before walking sections
//! top to bottom (fall-through) and
//! returns the run's result. The host-facing vocabulary a run is configured
//! with - the progress observer, the model and tool catalogs, the tool
//! contract - lives in the `shared-promptforge-api` crate
//! (`shared_promptforge_api::observe`, `shared_promptforge_api::models`,
//! `shared_promptforge_api::tools`), and the store handle a host seeds or
//! extracts comes from `shared-vfs` and `promptforge-vfs`.
//! [`execute::run`] takes an [`execute::RunContext`] carrying the
//! observer the correlated report records go to, and
//! `shared_promptforge_api::observe::NullObserver` is what a caller wanting
//! silence passes.
//! [`debug::DebugCapture`] is an opt-in raw request/response seam on the same
//! context; production hosts leave it unset.
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
//! use promptforge_api::{Prompt, promptforge_version};
//! use shared_promptforge_api::observe::NullObserver;
//!
//! let source = "---\nname: greeter\ndescription: says hi\npromptforge: 0\n---\n\n# Greeter\n\n## Say hi\n\nSay hello.\n\n```lua\nreturn models.infer(prose)\n```\n";
//!
//! // Version detection gates whether the runtime will accept the source.
//! assert_eq!(promptforge_version(source), Some(0));
//! assert_eq!(promptforge_version("plain text, no frontmatter"), None);
//!
//! let prompt = Prompt::parse(source, "doc-example", &NullObserver::default())?;
//! assert_eq!(prompt.title(), "Greeter");
//! assert_eq!(prompt.sections()[0].name(), "Say hi");
//! # Ok::<(), promptforge_api::ParseError>(())
//! ```
//!
//! Executing a parsed prompt goes through [`run`] with a [`RunContext`]
//! built from an [`Environment`] (which holds the optional picker, the
//! model catalog, and the tool catalog); the store handle rides on the
//! context, defaulting to the stock in-memory mount. That path can perform
//! gateway I/O, so it is shown as `no_run`:
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use promptforge_api::{Environment, Prompt, RunContext, RunResult};
//! use shared_promptforge_api::observe::NullObserver;
//!
//! let source = "---\nname: greeter\ndescription: says hi\npromptforge: 0\n---\n\n# Greeter\n\n## Say hi\n\nSay hello.\n\n```lua\nreturn models.infer(prose)\n```\n";
//! let prompt = Prompt::parse(source, "run-example", &NullObserver::default())?;
//!
//! // Capability-free agents use the default environment: no picker, empty
//! // catalogs.
//! let env = Environment::new();
//! let answer = env.run(&prompt, "", RunContext::new("run-example")).await;
//! let RunResult::Ok(text) = answer else {
//!     panic!("the greeter run succeeds: {answer:?}");
//! };
//! println!("{text}");
//! # Ok(())
//! # }
//! ```
//!
pub(crate) mod cancel;
pub mod capabilities;
pub mod client;
pub mod debug;
mod error;
pub mod execute;
pub(crate) mod fanout;
pub mod input;
pub(crate) mod lua;
pub(crate) mod model;
pub(crate) mod observe;
pub mod parser;
mod resolve;
pub(crate) mod store;
pub(crate) mod subst;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod tools;
pub(crate) mod untrusted;

pub(crate) use crate::error::{Error, Result};
pub(crate) use crate::tools::NearDuplicateDiagnostic;

pub use crate::capabilities::{CapabilityRegistry, RegistryError, RegistryErrorKind};
pub use crate::client::{CompletionError, CompletionErrorKind};
pub use crate::execute::{
    Environment, RequirementCheck, Requirements, RunContext, RunError, RunErrorKind, RunLimits,
    RunResult, SourceLocation, UnmetRequirement, run,
};
pub use crate::parser::{ParseError, ParseErrorKind, Prompt, promptforge_version};
