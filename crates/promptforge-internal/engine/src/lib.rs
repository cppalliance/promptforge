#![doc = include_str!("lib.md")]

pub(crate) mod cancel;
mod error;
mod execute;
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

// The crate root is the one path to the execution engine's items; the
// `execute` module stays private and every item that remains public inside it
// is re-exported here, so the `promptforge` facade names
// `promptforge_engine::X`, not a module path.
pub use crate::execute::{
    AnswerRecord, CapabilityConflict, ChatAnswerRecord, Effect, EffectAnswer, EffectId,
    EffectRecord, Environment, InputAnswerRecord, ModelBindings, RequirementCheck, Requirements,
    Run, RunContext, RunError, RunErrorKind, RunLimits, RunResult, SourceLocation, Step,
    StoreAnswerRecord, StoreError, StoreOp, StoreOutcome, ToolAnswerRecord, ToolBindings,
    UnmetRequirement, perform_store_op,
};
pub use crate::model::{CompletionError, CompletionErrorKind};
pub use crate::parser::{ParseError, ParseErrorKind, Prompt, promptforge_version};
