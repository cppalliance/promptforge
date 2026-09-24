//! Section lifecycle execution and fall-through.
//!
//! The host-facing design - the section walk, the host loop, reporting,
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
//! - `engine` - the walk-target resolution helpers.
//! - `environment` - the public [`Environment`], whose `prepare` fills slots
//!   against the host-supplied catalog; capability activation itself is the
//!   harness's, in `harness-capabilities`.
//! - `error` - the public [`RunError`] and its stable [`RunErrorKind`].
//! - `fill` - prepare's tool- and model-slot fill functions.
//! - `protocol` - the coroutine request/answer types for the yield/resume
//!   boundary.
//! - `requirements` - the preflight [`Requirements`] report.
//! - `run` - the host boundary: the `Run` state machine with its `step`,
//!   `resume`, and `cancel`, the `Step` it returns, and the effect vocabulary
//!   (the `Effect` a leaf arm issues, its serializable `EffectRecord`, and the
//!   `EffectAnswer` a host returns).
//! - `scheduler` - the chain-stack scheduler driving the coroutine protocol:
//!   the live H1 pass, the walk, call chains, fanout, and the `chat` and
//!   `tool_call` rounds the section-visible `models.loop` shim yields.
//! - `scope` - tool-scope validation and schema/dispatch preparation.
//! - `section_context` - the per-section `SectionContext` frame the
//!   scheduler's chains construct, run, and tear down.
//! - `section_vm` - the section VM setup half shared by the walk and the
//!   fanout arm.
//! - `support` - shared helpers.
//! - `tools` - the nested-inference round's answer.

mod bindings;
mod config;
pub(crate) mod context;
mod engine;
mod environment;
mod error;
mod fill;
pub(crate) mod protocol;
mod requirements;
pub(crate) mod run;
pub(crate) mod scheduler;
mod scope;
mod section_context;
pub(crate) mod section_vm;
mod support;
mod tools;

// Public API surface.
pub use bindings::{ModelBindings, ToolBindings};
pub use config::{RunContext, RunLimits};
pub use environment::Environment;
pub use error::{RunError, RunErrorKind, SourceLocation};
pub use requirements::{CapabilityConflict, RequirementCheck, Requirements, UnmetRequirement};
pub use run::{
    AnswerRecord, ChatAnswerRecord, Effect, EffectAnswer, EffectId, EffectRecord,
    InputAnswerRecord, Run, Step, StoreAnswerRecord, ToolAnswerRecord,
};
// The store vocabulary a `Store` effect holds and its answer returns:
// named here so a host's store performer can be written against this one
// crate without reaching behind it.
pub use promptforge_lua::{StoreOp, StoreOutcome};
pub use promptforge_store::StoreError;

/// Performs one store operation through `access`: the work behind an
/// [`Effect::Store`], for a host's store performer. The engine's own
/// store facade runs the operation, so a host answers a store effect
/// exactly as the engine's test drivers do; `access` is used as given,
/// and nothing here derives, widens, or retains store scope from it.
///
/// Synchronous, because the VFS is synchronous by design; a host runs it
/// off its async executor.
///
/// # Errors
/// Returns the store's own failure for the operation (path validation,
/// not-found, anchor, range, write-race, or backend failure), which the
/// engine raises at the author's call site when the answer is resumed.
pub fn perform_store_op(
    access: &promptforge_vfs::Access,
    op: StoreOp,
) -> std::result::Result<StoreOutcome, StoreError> {
    crate::lua::run_store_op(&crate::store::Store::new(access), op)
}

/// What the run produced. Domain outcomes (including "the prompt
/// declined") are values, not thrown errors: the variant is for code, the
/// payload is for humans and models. A host reads it out of
/// [`Step::Done`].
///
/// # Outcomes
/// - [`RunResult::Ok`] - the run completed with its final text.
/// - [`RunResult::Cancelled`] - the host cancelled the run.
/// - [`RunResult::Failure`] - the run failed; the [`RunError`]'s
///   [`kind`](RunError::kind) classifies the failure by condition:
/// - [`RunErrorKind::Parse`] - a prompt/frontmatter or compiled Lua region was
///   invalid.
/// - [`RunErrorKind::Version`] - the prompt declared an unsupported
///   `promptforge:` major.
/// - [`RunErrorKind::Binding`] - a `tools.bind`/`models.bind` capability could
///   not be bound, was absent, or clashed.
/// - [`RunErrorKind::Completion`] - a model completion failed at the transport,
///   backend, or decode layer.
/// - [`RunErrorKind::Tool`] - a dispatched tool failed, was out of scope, or the
///   tool loop did not converge.
/// - [`RunErrorKind::Lua`] - a section's Lua phase failed to run or return a
///   usable value.
/// - [`RunErrorKind::Quota`] - a Lua host resource quota (log events, log bytes,
///   or instructions) was exhausted.
/// - [`RunErrorKind::ContextExhausted`] - the selected compactor exhausted the
///   model's context window.
/// - [`RunErrorKind::Input`] - the host's input broker failed a `user_input`
///   request.
/// - [`RunErrorKind::Substitution`] - a `{{ }}` prose substitution failed.
/// - [`RunErrorKind::Store`] - a run-scoped store operation failed.
/// - [`RunErrorKind::Determinism`] - two live execution identities claimed
///   one store path; the run terminated on the spot, uncatchably from Lua.
/// - [`RunErrorKind::Cancelled`] - the host cancelled the run (mid-run
///   classification only; the interface reports [`RunResult::Cancelled`]).
/// - [`RunErrorKind::Internal`] - an internal invariant failed.
/// - [`RunErrorKind::RequirementsUnmet`] - an H1 assertion or model
///   requirement the environment cannot satisfy.
#[derive(Debug)]
pub enum RunResult {
    /// The run completed with its final text. Mirrors `Result` vocabulary,
    /// so patterns need `RunResult::Ok` qualification wherever `Result` is
    /// also in scope.
    Ok(String),
    /// The host cancelled the run.
    Cancelled,
    /// The run failed; the typed error classifies the failure.
    Failure(RunError),
}

/// One task's history out of a host's event log: every event whose
/// provenance names `task` with a sequence number after `last` (every one
/// of the task's events when `last` is `None`), in log order - which is
/// sequence order within one task, since a task's events are pushed in
/// the order its counter stamps them. The answer to a
/// [`Effect::TaskEvents`] read, shared by the test drivers.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn task_history(
    log: &[promptforge_types::event::Event],
    task: &promptforge_types::ids::TaskId,
    last: Option<u32>,
) -> Vec<promptforge_types::event::Event> {
    log.iter()
        .filter(|event| {
            let provenance = event.provenance();
            provenance.task == *task && last.is_none_or(|last| provenance.seq > last)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
