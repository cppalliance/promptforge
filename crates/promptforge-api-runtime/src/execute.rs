//! Section lifecycle execution and fall-through.
//!
//! The run walks top-level sections in file order, creating one isolated
//! section VM for each. The VM is fully equipped (host values, store, log,
//! control globals) before the shared Lua library replays as the section's
//! first chunk, then ordered section blocks use that same VM. Prose never
//! infers: each prose block stashes the pending Markdown buffer, and the
//! next Lua block reads it as its fresh read-only lazy `prose` template.
//! A scalar Lua return ends the chain it fires in.
//!
//! Running off the last section ends the run: the result is the last
//! scalar return, else a generic completion.
//!
//! The walk is level-independent and never descends on its own: a jump to a
//! child heading starts a child-level walk over the jumper's children under
//! the same rules, and the parent walk resumes after the jumper when that
//! level exhausts.
//!
//! One run-scoped store handle travels with the run's [`RunContext`] (the
//! stock handle by default), shared by
//! every section, so
//! bulk state persists across the context-clearing transitions even though a
//! section's Lua state never does.
//!
//! A run reports itself as it goes, as values: every boundary - the run's
//! start and end, each section, model turn, tool call, and
//! harness-mediated store operation - is an
//! [`Event`](promptforge_api_types::event::Event) pushed into the run's
//! event buffer, stamped with the
//! [`Provenance`](promptforge_api_types::ids::Provenance) of the chain that
//! reported it (its nearest enclosing task and that task's next sequence
//! number). Every `step` of the run drains the buffer and returns the
//! batch to the host, which appends it to its log. Reporting is a side
//! channel and never a decision: nothing the run does depends on who
//! reads its events.
//!
//! Rust installs the run's filled tool and model slots - bound at prepare
//! from the frontmatter against the host-supplied catalog - into each
//! section VM. Prompt-wide aliases and section additions form the
//! effective model-visible scope, whose tools are advertised under their
//! local aliases from the descriptor each binding carries; a call is
//! issued as a `ToolCall` effect naming the tool's id, and the host
//! resolves the implementation.
//!
//! Lua `call()` starts a contained chain at a visible section (fresh VM,
//! recursion capped at 8): the chain runs from the target
//! with every normal walk rule - fall-through, jumps, child
//! chains - and the outer walk never moves while it runs. When the chain
//! ends (its level exhausts or a return fires), its final text is the call's
//! return value; a return ends only the chain it fires in.
//! Lua `jump(target)` transfers control to a named section.
//!
//! # Runtime
//!
//! The engine is a state machine (`run::Run`): it performs no I/O and
//! awaits nothing. Section Lua yields request messages to the chain-stack
//! scheduler, which turns each leaf request into an effect value the
//! host performs and answers, so a run needs no runtime at all - the
//! host's loop performs on whatever it likes, and the serial driver in
//! `test_support` runs any prompt on the calling thread, host calls
//! included. Concurrency (a fanout's arms) comes from interleaving chains
//! at their effect boundaries, not from worker threads.
//!
//! # Module layout
//!
//! The run's outcome type ([`RunResult`]) lives here; the rest is split
//! into focused private children: `error` (the public [`RunError`]),
//! `config` ([`RunContext`]/[`RunLimits`]), `environment` (the public
//! [`Environment`], whose `prepare` fills slots against the host-supplied
//! catalog; capability activation itself is the harness's, in
//! `harness-capabilities`), `requirements` (the preflight
//! [`Requirements`] report), `context` (the ambient `RunState` run
//! state), `tools` (the nested-inference round's answer),
//! `section_vm` (the section VM setup half shared by the walk and
//! the fanout arm), `section_context` (the per-section `SectionContext`
//! frame the scheduler's chains construct, run, and tear down),
//! `engine` (the walk-target
//! resolution helpers), `protocol` (the coroutine request/answer types
//! for the yield/resume boundary), `run` (the host boundary: the `Run`
//! state machine with its `step`, `resume`, and `cancel`, the `Step` it
//! returns, and the effect vocabulary - the `Effect` a leaf arm issues,
//! its serializable `EffectRecord`, and the `EffectAnswer` a host
//! returns), `scheduler` (the chain-stack scheduler driving the coroutine
//! protocol: the live H1 pass, the walk, call chains, fanout, and the
//! `chat` and `tool_call` rounds the section-visible `models.loop` shim
//! yields), `scope` (tool-scope validation and schema/dispatch
//! preparation), and `support` (shared helpers).

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
// The store vocabulary a `Store` effect carries and its answer returns:
// named here so a host's store performer can be written against this one
// door without reaching behind it.
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
    access: &shared_vfs::Access,
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
    log: &[promptforge_api_types::event::Event],
    task: &promptforge_api_types::ids::TaskId,
    last: Option<u32>,
) -> Vec<promptforge_api_types::event::Event> {
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
