//! Section lifecycle execution and fall-through.
//!
//! The public design - the section walk, the Harness loop, reporting,
//! tool binding, and running without a runtime - is documented on the
//! `promptforge` facade's crate page and its role modules.
//!
//! # Module layout
//!
//! The run's outcome type ([`RunResult`]) is defined here; the rest lives in
//! focused private children:
//!
//! - `bindings` - the run's journaled [`ModelBindings`] and [`ToolBindings`].
//! - `config` - the public [`RunContext`] and [`RunLimits`].
//! - `context` - the ambient `RunState` run state.
//! - `environment` - the public [`Environment`], whose `prepare` fills slots
//!   against the caller's catalog; Plugin activation itself is the
//!   Harness's, in `harness-plugins`.
//! - `error` - the public [`RunError`] and its stable [`RunErrorKind`].
//! - `fill` - prepare's tool- and model-slot fill functions.
//! - `protocol` - the coroutine request/answer types for the yield/resume
//!   boundary.
//! - `requirements` - the preflight [`Requirements`] report.
//! - `run` - the Harness boundary: the `Run` state machine with its `step`,
//!   `resume`, and `cancel`, the `Step` it returns, and the effect vocabulary
//!   (the `Effect` a leaf arm issues, its serializable `EffectRecord`, and the
//!   `EffectAnswer` the Harness returns).
//! - `scheduler` - the chain scheduler driving the coroutine protocol:
//!   the live H1 pass, the walk, call chains, fanout, and the `chat` and
//!   `tool_call` rounds the section-visible `models.loop` shim yields.
//! - `scope` - tool-scope validation and schema/dispatch preparation.
//! - `section_context` - the per-section `SectionContext` frame the
//!   scheduler's chains construct, run, and tear down.
//! - `section_vm` - the section VM setup half shared by the walk and the
//!   fanout arm.
//! - `support` - shared helpers.
//! - `tools` - the nested-inference round's answer.
//! - `walk_target` - the walk-target resolution helpers.

mod bindings;
mod config;
pub(crate) mod context;
mod environment;
mod error;
mod fill;
pub(crate) mod protocol;
mod requirements;
mod run;
pub(crate) mod scheduler;
mod scope;
mod section_context;
pub(crate) mod section_vm;
mod support;
mod tools;
mod walk_target;

// Public API surface.
pub use bindings::{ModelBindings, ToolBindings};
pub use config::{RunContext, RunLimits};
pub use environment::Environment;
pub use error::{RunError, RunErrorKind, SourceLocation};
pub use requirements::{
    MissingService, PluginConflict, RequirementCheck, Requirements, UnmetRequirement,
};
pub use run::{
    AnswerRecord, ChatAnswerRecord, Effect, EffectAnswer, EffectId, EffectRecord, Round, Run, Step,
    ToolAnswerRecord, ToolCallOrigin, ToolCaller,
};
// The store vocabulary a `Vfs` effect holds and its answer returns, for
// the Engine's own store handling; other crates name it from `promptforge_lua`.
use promptforge_lua::{VfsOp, VfsOutcome};

/// Performs the store operation of an [`Effect::Vfs`] and returns its answer.
///
/// `access` is the store view the effect carries. It is derived from the
/// chain's capability when the effect is dispatched. Each [`VfsOp`] maps
/// onto one `Access` call on that view, so the caller answers a `Vfs`
/// effect exactly as the Engine's own test drivers do. Line bounds arrive
/// as `i64` and convert to `usize`. A read that gives an `end` and omits
/// `start` is refused as an invalid range.
///
/// The function is synchronous because the store is synchronous by
/// design. The caller can run it inline, on the thread that runs its
/// effect loop.
///
/// # Errors
/// Returns the store's own structured failure for the operation (path
/// validation, not-found, anchor, range, conflict, or backend failure).
/// When the caller resumes the run with that answer, the Engine raises
/// the failure at the prompt author's call site.
pub fn perform_vfs_op(
    access: &promptforge_vfs::Access,
    op: VfsOp,
) -> std::result::Result<VfsOutcome, promptforge_vfs::VfsError> {
    crate::lua::run_store_op(access, op)
}

/// The outcome of a run, which the caller reads out of [`Step::Done`].
///
/// Every outcome is a value of this enum. That includes a prompt that
/// declines its task. The variant tells code what happened, and the
/// payload explains it to people and models.
///
/// # Outcomes
/// - [`RunResult::Ok`] - the run completed with its final text.
/// - [`RunResult::Cancelled`] - the caller cancelled the run.
/// - [`RunResult::Failure`] - the run failed.
///
/// A failure's [`RunError`] has a [`kind`](RunError::kind) that classifies
/// the failure by condition:
/// - [`RunErrorKind::Parse`] - the prompt, its frontmatter, or a compiled
///   Lua region was invalid.
/// - [`RunErrorKind::Version`] - the prompt declared an unsupported
///   `promptforge:` major version.
/// - [`RunErrorKind::Binding`] - a tool or model capability failed to bind
///   or was missing.
/// - [`RunErrorKind::Completion`] - a model completion failed at the transport,
///   backend, or decode layer.
/// - [`RunErrorKind::Tool`] - a dispatched tool failed, was out of scope, or the
///   tool loop failed to converge.
/// - [`RunErrorKind::Lua`] - a section's Lua phase failed to run or return a
///   usable value.
/// - [`RunErrorKind::Quota`] - a Lua resource quota (log events, log bytes,
///   or instructions) was exhausted.
/// - [`RunErrorKind::ContextExhausted`] - the selected compactor exhausted the
///   model's context window.
/// - [`RunErrorKind::Substitution`] - a `{{ }}` prose substitution failed.
/// - [`RunErrorKind::Vfs`] - a run-scoped store operation failed, or the
///   run's handle lacks a store declaration.
/// - [`RunErrorKind::Determinism`] - two live execution identities claimed
///   one store path. The run ended on the spot, and Lua code cannot catch
///   this failure.
/// - [`RunErrorKind::Cancelled`] - the caller cancelled the run. This kind
///   only classifies errors raised while the run is still going. The run
///   itself ends in [`RunResult::Cancelled`].
/// - [`RunErrorKind::Internal`] - an internal invariant failed.
/// - [`RunErrorKind::RequirementsUnmet`] - a missing required Plugin,
///   a service the caller left out, a Plugin conflict, a model
///   requirement a bound model fails to meet (a context minimum or a hard
///   keyword), or an H1 block that failed the prompt's hard gate.
#[derive(Debug)]
pub enum RunResult {
    /// The run completed with its final text. The name matches
    /// `Result::Ok`, so patterns must write `RunResult::Ok` wherever
    /// `Result` is also in scope.
    Ok(String),
    /// The caller cancelled the run.
    Cancelled,
    /// The run failed. The error's kind classifies the failure.
    Failure(RunError),
}

#[cfg(test)]
mod tests;
