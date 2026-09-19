//! PromptForge runtime core.
//!
//! This crate holds the pieces that turn a prompt markdown file into a model
//! call: the [`parser`] that reads the file into a [`parser::Prompt`], the
//! [`client`] that talks to an `OpenAI`-compatible chat completions endpoint, and
//! [`execute`] that runs H1 once with live resolution before walking sections
//! top to bottom (fall-through) and
//! returns the run's result. The host-facing vocabulary a run is configured
//! with - the progress observer, the model and tool catalogs, the tool
//! contract - lives in the `promptforge-api-types` crate, re-exported here
//! as [`types`] (`types::observe`, `types::models`, `types::tools`), so a
//! host depends on this one crate alone. The store handle a host seeds or
//! extracts comes from `shared-vfs` and `promptforge-vfs`.
//! [`execute::run`] takes an [`execute::RunContext`] carrying the
//! observer the correlated report records go to, and
//! `types::observe::NullObserver` is what a caller wanting
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
//! use promptforge_api_runtime::types::observe::NullObserver;
//! use promptforge_api_runtime::{Prompt, promptforge_version};
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
//! # Ok::<(), promptforge_api_runtime::ParseError>(())
//! ```
//!
//! Executing a parsed prompt goes through [`run`] with a [`RunContext`]
//! built from an [`Environment`] (which holds the capability registry and
//! the deployment's client); the store handle rides on the
//! context, defaulting to the stock in-memory mount. That path can perform
//! gateway I/O, so it is shown as `no_run`:
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use promptforge_api_runtime::types::observe::NullObserver;
//! use promptforge_api_runtime::types::timestamp::Timestamp;
//! use promptforge_api_runtime::{Environment, Prompt, RunContext, RunResult};
//!
//! let source = "---\nname: greeter\ndescription: says hi\npromptforge: 0\n---\n\n# Greeter\n\n## Say hi\n\nSay hello.\n\n```lua\nreturn models.infer(prose)\n```\n";
//! let prompt = Prompt::parse(source, "run-example", &NullObserver::default())?;
//!
//! // Capability-free agents use the default environment: no registry, empty
//! // catalogs. The host draws the run's seed and stamps its start: the
//! // engine reads neither the OS RNG nor the clock.
//! let env = Environment::new();
//! let seed: u64 = 0x5eed; // a CSPRNG draw in a real host
//! let started_at = Timestamp::from_unix_millis(1_700_000_000_000);
//! let answer = env.run(&prompt, "", RunContext::new("run-example", seed, started_at)).await;
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
pub(crate) mod store;
pub(crate) mod subst;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub(crate) mod tools;
pub(crate) mod untrusted;

pub(crate) use crate::error::{Error, Result};

pub use crate::capabilities::{CapabilityRegistry, RegistryError, RegistryErrorKind, Web};
pub use crate::client::{CompletionError, CompletionErrorKind};
pub use crate::execute::{
    Effect, EffectAnswer, EffectId, EffectRecord, Environment, RequirementCheck, Requirements, Run,
    RunContext, RunError, RunErrorKind, RunLimits, RunResult, SourceLocation, Step,
    UnmetRequirement, run,
};
pub use crate::parser::{ParseError, ParseErrorKind, Prompt, promptforge_version};
pub use promptforge_api_types as types;
