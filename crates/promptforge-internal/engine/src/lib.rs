#![doc = include_str!("lib.md")]
//!
//! ## Invariants
//!
//! - May depend on: `promptforge-types`, `promptforge-lua`,
//!   `promptforge-parser`, `promptforge-vfs`, and
//!   `promptforge-model-client`. `cargo test -p build-xtask` enforces the
//!   product and container boundaries.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.

pub(crate) mod cancel;
mod error;
mod execute;
pub(crate) mod heading_address;
pub(crate) mod lua;
pub(crate) mod model;
pub(crate) mod parser;
pub(crate) mod subst;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub(crate) mod tools;
pub(crate) mod untrusted;

pub(crate) use crate::error::{Error, Result};

// The crate root is the one path to this crate's items; the
// `execute` module stays private and every item that remains public inside it
// is re-exported here, so the `promptforge` facade names
// `promptforge_engine::X`, not a module path.
pub use crate::execute::{
    AnswerRecord, CapabilityConflict, ChatAnswerRecord, Effect, EffectAnswer, EffectId,
    EffectRecord, Environment, MissingService, ModelBindings, RequirementCheck, Requirements, Run,
    RunContext, RunError, RunErrorKind, RunLimits, RunResult, SourceLocation, Step,
    ToolAnswerRecord, ToolBindings, ToolCallOrigin, ToolCaller, UnmetRequirement, perform_vfs_op,
};
