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
//! batch to the host; the in-crate tokio driver behind [`run`] forwards
//! it to the [`RunContext`] observer and debug capture through the
//! `events_to_observer` adapter. Reporting is a side channel and never a
//! decision, so passing
//! [`NullObserver`](promptforge_api_types::observe::NullObserver) changes
//! nothing but the silence.
//!
//! Rust installs the run's filled tool and model slots - bound at prepare
//! from the frontmatter - into each section VM. Prompt-wide aliases and
//! section additions form the effective model-visible scope, whose
//! concrete tools are advertised under their local aliases and dispatched
//! through the implementation each binding carries.
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
//! host performs and answers, so a run needs no particular Tokio runtime
//! flavor - the in-crate driver behind [`run`] performs on whatever
//! runtime the caller is on, and a current-thread runtime runs any
//! prompt, host calls included. Concurrency (a fanout's arms) comes from
//! interleaving chains at their effect boundaries, not from worker
//! threads.
//!
//! # Module layout
//!
//! The orchestration boundary ([`run`]) lives here; the rest is split into
//! focused private children: `error` (the public [`RunError`]), `config`
//! ([`RunContext`]/[`RunLimits`]), `environment` (the public
//! [`Environment`]), `requirements` (the preflight
//! [`Requirements`] report), `context` (the ambient `RunState` run
//! state), `event_buffer` (the run-level event buffer and the
//! task-scoped emitter every report goes through), `events_to_observer`
//! (the adapter replaying drained events onto the host's observer and
//! capture), `gateway` (client acquisition and the live H1 resolution
//! inputs),
//! `tools` (the nested-inference round's answer),
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
//! yields), `tokio_driver` (the in-crate tokio host that performs a
//! run's effects with the configured client, tools, and broker), `scope`
//! (tool-scope validation and schema/dispatch preparation), and
//! `support` (shared helpers).

mod bindings;
mod config;
mod context;
mod engine;
mod environment;
mod error;
mod event_buffer;
mod events_to_observer;
mod fill;
mod gateway;
pub(crate) mod protocol;
mod requirements;
pub(crate) mod run;
mod scheduler;
mod scope;
mod section_context;
pub(crate) mod section_vm;
mod support;
pub(crate) mod tokio_driver;
mod tools;

// Public API surface.
pub use bindings::{ModelBindings, ToolBindings};
pub use config::{RunContext, RunLimits};
pub use environment::Environment;
pub use error::{RunError, RunErrorKind, SourceLocation};
pub use requirements::{CapabilityConflict, RequirementCheck, Requirements, UnmetRequirement};

use std::sync::Arc;

use run::Run;
use tokio_driver::TokioDriver;

use crate::Error;
use crate::parser::Prompt;

/// What the run produced. Domain outcomes (including "the prompt
/// declined") are values, not thrown errors: the variant is for code, the
/// payload is for humans and models.
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

/// Executes a parsed prompt and returns its final text.
///
/// H1 is section 0: its Lua and prose blocks run once in source order with
/// the same surface every section gets; its only privilege is `argv`
/// writability - every other section reads the value H1 left behind, frozen.
/// If H1 does not return, the H2 section walk runs and its final text is
/// returned.
///
/// The free `run` receives an already-prepared [`RunContext`] and has
/// nothing to prepare from: a context that never passed through
/// [`Environment::prepare`] runs capability-free (empty tool and model
/// sets). Hosts normally go through [`Environment::run`], the
/// zero-burden path.
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
///
/// # Examples
/// A no-network prompt whose walk makes a nested host call: `call` is a
/// structural request the scheduler drives on the run's one thread, so the
/// current-thread runtime below runs the whole prompt, host calls included:
/// ```
/// use promptforge_api_runtime::execute::{RunContext, RunResult, run};
/// use promptforge_api_runtime::parser::Prompt;
/// use promptforge_api_types::observe::NullObserver;
///
/// let source = concat!(
///     "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n",
///     "# Title\n\n",
///     "## Calls\n\n",
///     "```lua\nreturn call('## Answers')\n```\n\n",
///     "## Answers\n\n",
///     "```lua\nreturn 'hello'\n```\n",
/// );
/// let prompt = Prompt::parse(source, "doc-example", &NullObserver::default())?;
/// let runtime = tokio::runtime::Builder::new_current_thread().build()?;
/// let output = runtime.block_on(run(&prompt, "", RunContext::new("doc-example")));
/// let RunResult::Ok(text) = output else {
///     panic!("the doc example run succeeds: {output:?}");
/// };
/// assert_eq!(text, "hello");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Runtime
/// A run needs no particular Tokio runtime flavor. The engine itself
/// awaits nothing: every chain step - all Lua, the walk, call chains, and
/// fanout joins - runs inside one `Run::step`, and suspending Lua host
/// calls (`models.infer`, `call`, `fanout`) are coroutine yields the
/// scheduler turns into effects, so no host call parks a worker thread.
/// The tokio loop behind this function performs those effects with the
/// context's client, tools, and broker and feeds the answers back.
/// Concurrency (a fanout's arms) comes from interleaving chains at their
/// effect boundaries, not from threads; on a multi-thread runtime only
/// the performers, which never touch Lua or scheduler state, may run on
/// other workers.
pub async fn run(prompt: &Prompt, args: &str, ctx: RunContext) -> RunResult {
    // The caller-supplied client honors the run's HTTP limits, as a
    // lazily built environment client does.
    let limits = ctx.limits;
    let mut ctx = ctx;
    let client = ctx
        .client
        .take()
        .map(|client| client.with_request_limits(limits.timeout(), limits.response_bytes()));
    let run = Run::new(Arc::new(prompt.clone()), args, ctx);
    // Boxed: the driver future carries the whole step machinery, and
    // `run`'s own future must stay small for its callers (the workspace's
    // large-futures lint gates every one of them).
    let mut driver = TokioDriver::over(run, client);
    let result = Box::pin(driver.drive()).await;
    match result {
        Ok(text) => RunResult::Ok(text),
        Err(Error::Interrupted) => RunResult::Cancelled,
        Err(error) => RunResult::Failure(RunError::from(error)),
    }
}

#[cfg(test)]
mod tests;
