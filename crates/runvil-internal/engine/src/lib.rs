//! The Engine's runtime core: the [`Run`] state machine that executes a
//! parsed prompt.
//!
//! A run executes the Lua under the `#` title once, then walks the sections
//! top to bottom. It issues every model round, tool call, store operation,
//! and timer as an [`Effect`] the Harness performs and answers, and reports
//! every boundary as an event returned from `step`. Other crates reach these
//! items through the `runvil` facade.
//!
//! ## Invariants
//!
//! - May depend on: `runvil-types`, `runvil-lua`,
//!   `runvil-parser`, `runvil-vfs`, and
//!   `runvil-model-client`. `cargo test -p build-xtask` enforces the
//!   product and container boundaries.

mod cancel;
mod error;
mod execute;
mod heading_address;
mod lua;
mod model;
mod parser;
mod subst;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
mod tools;
mod untrusted;

use crate::error::{Error, Result};

// The crate root is the one path to this crate's items; the
// `execute` module stays private and every item that remains public inside it
// is re-exported here, so the `runvil` facade names
// `runvil_engine::X`, not a module path.
pub use crate::execute::{
    AnswerRecord, ChatAnswerRecord, Effect, EffectAnswer, EffectId, EffectRecord, FailureRecord,
    MissingService, ModelBindings, RequirementCheck, Requirements, Round, Run, RunContext,
    RunError, RunErrorKind, RunLimits, RunResult, SourceLocation, Step, ToolAnswerRecord,
    ToolCallOrigin, ToolCaller, UnavailablePlugin, UnmetRequirement, perform_vfs_op,
};
