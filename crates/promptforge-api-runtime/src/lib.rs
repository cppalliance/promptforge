#![doc = include_str!("lib.md")]

pub(crate) mod cancel;
mod error;
pub mod execute;
pub(crate) mod fanout;
pub mod input;
pub(crate) mod lua;
pub mod model;
pub mod parser;
pub(crate) mod store;
pub(crate) mod subst;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub(crate) mod tools;
pub(crate) mod untrusted;

pub(crate) use crate::error::{Error, Result};

pub use crate::execute::{
    AnswerRecord, Effect, EffectAnswer, EffectId, EffectRecord, Environment, RequirementCheck,
    Requirements, Run, RunContext, RunError, RunErrorKind, RunLimits, RunResult, SourceLocation,
    Step, UnmetRequirement,
};
pub use crate::model::{CompletionError, CompletionErrorKind};
pub use crate::parser::{ParseError, ParseErrorKind, Prompt, promptforge_version};
pub use promptforge_api_types as types;
