//! The run: the Engine's Harness boundary, four methods exchanging effects and
//! events as values. The Harness loop is documented on the `promptforge`
//! facade's crate page and its `effect` and `cancel` modules.
//!
//! The effect vocabulary itself - [`Effect`] and its [`Round`], its
//! serializable [`EffectRecord`], [`EffectAnswer`], and [`EffectId`] - is
//! defined in the `effect` child module and is re-exported here.

use std::sync::Arc;

use promptforge_types::event::Event;
use promptforge_types::ids::Provenance;

mod effect;

pub use effect::{
    AnswerRecord, ChatAnswerRecord, Effect, EffectAnswer, EffectId, EffectRecord, Round,
    ToolAnswerRecord, ToolCallOrigin, ToolCaller,
};

use crate::cancel::CancelHandle;
use crate::parser::{ParseErrorKind, Prompt};
use crate::{Error, Result};

use super::RunResult;
use super::config::RunContext;
use super::context::RunState;
use super::error::RunError;
use super::scheduler::Scheduler;

/// What one [`Run::step`] produced.
#[derive(Debug)]
pub enum Step {
    /// The run continues. `effects` are the leaf effects this step
    /// issued, in issue order, each with the provenance of the task that
    /// built it; an empty list means every chain waits on an effect
    /// already issued. `events` are the reports the step made, in order.
    Pending {
        /// The effects the Harness performs and answers through
        /// [`Run::resume`].
        effects: Vec<(EffectId, Provenance, Effect)>,
        /// The events the step reported.
        events: Vec<Event>,
    },
    /// The run is over: its result and the last events, the run's own end
    /// boundary among them. Returned only once every issued effect has
    /// been answered.
    Done {
        /// The run's outcome.
        result: RunResult,
        /// The events reported since the previous step.
        events: Vec<Event>,
    },
}

/// One run of one prompt, driven by the Harness through
/// [`step`](Self::step) and [`resume`](Self::resume).
///
/// `Run` is `Send`: one caller drives it at a time, and the thread may
/// change between calls. It owns its prompt through an `Arc`, so the Harness
/// keeps parsing once and running many times.
///
/// # Examples
/// A prompt whose only section returns a literal issues no effect, so the
/// Harness drives it to `Done` in one step:
/// ```
/// use std::sync::Arc;
///
/// use promptforge::timestamp::Timestamp;
/// use promptforge::{Prompt, Run, RunContext, RunResult, Step};
///
/// let source = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n# Title\n\n## Only\n\n```lua\nreturn 'hello'\n```\n";
/// let (prompt, _parse_events) = Prompt::parse(source, "doc-example");
/// let prompt = prompt?;
/// let ctx = RunContext::new("doc-example", 1, Timestamp::UNIX_EPOCH);
/// let mut run = Run::new(Arc::new(prompt), "", ctx);
/// let Step::Done { result: RunResult::Ok(text), .. } = run.step() else {
///     panic!("the literal run is done at once");
/// };
/// assert_eq!(text, "hello");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Run {
    /// The scheduler, present unless construction failed.
    scheduler: Option<Scheduler>,
    /// A construction failure, delivered as the first step's `Done`.
    stillborn: Option<Error>,
    /// The run's cancel flag, shared with every chain's VM hook.
    cancel: CancelHandle,
}

impl std::fmt::Debug for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Run")
            .field("live", &self.scheduler.is_some())
            .field("stillborn", &self.stillborn)
            .field("cancel", &self.cancel)
            .finish()
    }
}

impl Run {
    /// Builds the run of `prompt` with `args` under `ctx`. A context that
    /// never passed through
    /// [`Environment::prepare`](super::Environment::prepare) runs
    /// capability-free (empty tool and model sets). A prompt without a
    /// supported `promptforge:` version, or a handle that declares no
    /// store or whose store backend fails the probe, yields a run whose
    /// first `step` is `Done` with the failure.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the public API takes the context by value: Run::new owns the run's inputs"
    )]
    #[must_use]
    pub fn new(prompt: Arc<Prompt>, args: &str, ctx: RunContext) -> Run {
        // The context's one flag, held here so a stillborn run still has
        // the handle `cancel` and `cancel_handle` name.
        let cancel = ctx.cancel.clone();
        match prepare_state(prompt, args, &ctx) {
            Ok(state) => Self::from_state(state),
            Err(error) => Run {
                scheduler: None,
                stillborn: Some(error),
                cancel,
            },
        }
    }

    /// Builds the run over an assembled context: the constructor the
    /// in-crate drivers and the suites use when they shape the context
    /// themselves.
    #[must_use]
    pub(crate) fn from_state(state: RunState) -> Run {
        let cancel = state.cancel().clone();
        Run {
            scheduler: Some(Scheduler::new(state)),
            stillborn: None,
            cancel,
        }
    }

    /// Drains the ready queue and returns what the run issued and
    /// reported: [`Step::Pending`] while any chain waits on an answer,
    /// [`Step::Done`] once the run is over and every issued effect is
    /// answered. A step after `Done` is a Harness error and reports an
    /// internal failure.
    ///
    /// Infallible by design: a run's failures are values in
    /// [`RunResult::Failure`], so the Harness drives the loop and owns the
    /// retry policy without catching a panic.
    pub fn step(&mut self) -> Step {
        if let Some(error) = self.stillborn.take() {
            return Step::Done {
                result: RunResult::Failure(RunError::from(error)),
                events: Vec::new(),
            };
        }
        match self.scheduler.as_mut() {
            Some(scheduler) => scheduler.step(),
            None => Step::Done {
                result: RunResult::Failure(RunError::from(Error::internal(
                    "a run that failed to start cannot be stepped again",
                ))),
                events: Vec::new(),
            },
        }
    }

    /// Applies one effect's answer: the parked chain resumes with it (or
    /// with a cancelled error for [`EffectAnswer::Dropped`]) and is
    /// re-queued for the next `step`; the round's events are buffered for
    /// that step. Each answer must match the kind of the effect it answers -
    /// a chat answer for a `Chat` effect, a tool answer for a `ToolCall`,
    /// and so on; a mismatch is an internal error that ends the run. An
    /// answer for an effect whose chain stopped waiting is discarded. An
    /// answer for an id the run never issued, or a second answer for one
    /// effect, is likewise an internal error that ends the run.
    ///
    /// Infallible by design: a bad answer ends the run with a
    /// [`RunResult::Failure`] rather than returning an error or panicking.
    pub fn resume(&mut self, id: EffectId, answer: EffectAnswer) {
        if let Some(scheduler) = self.scheduler.as_mut() {
            scheduler.resume(id, answer);
        }
    }

    /// Sets the run's cancel flag. Running Lua observes it from its
    /// instruction hook; the next `step` tears every chain down and, once
    /// the outstanding effects are answered, reports the run as cancelled.
    ///
    /// Cancellation is a request that later steps finish: the Harness answers
    /// each effect it abandons with [`EffectAnswer::Dropped`]. Infallible by
    /// design, like `step` and `resume`.
    pub fn cancel(&mut self) {
        self.cancel.cancel();
    }

    /// The run's cancel flag, for the Harness to cancel from another thread.
    #[must_use]
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }

    /// Whether the run's outcome is decided: its end boundary has been
    /// reported (or it never started) and every effect still out is an
    /// orphan whose answer only `Done` waits on. The Harness reads this after
    /// a `Pending` step to learn it may drop what it holds, so control
    /// never depends on the events, which are a report and not a decision.
    #[must_use]
    pub fn decided(&self) -> bool {
        self.scheduler.as_ref().is_none_or(Scheduler::decided)
    }

    /// The scheduler behind the run, for the suites that inspect its
    /// arena.
    #[cfg(test)]
    pub(crate) fn scheduler_for_test(&mut self) -> &mut Scheduler {
        self.scheduler
            .as_mut()
            .expect("a run built from a state holds its scheduler")
    }
}

/// Assembles the run state from the Harness's context, checking the version
/// gate, the shared library, and the declared store in that order.
///
/// # Errors
/// Returns [`Error::UnsupportedVersion`] or a structural parse error for a
/// prompt that is not a supported promptforge prompt, the Lua error when
/// the empty shared chunk cannot compile, or [`Error::Store`] when the
/// handle declares no store or the store's backend fails the probe.
fn prepare_state(prompt: Arc<Prompt>, args: &str, ctx: &RunContext) -> Result<RunState> {
    match prompt.frontmatter().promptforge() {
        Some(0) => {}
        Some(other) => return Err(Error::UnsupportedVersion(other)),
        None => {
            return Err(Error::parse(
                ParseErrorKind::Structure,
                "not a promptforge prompt: no promptforge version",
            )
            .with_prompt_name(prompt.frontmatter().name()));
        }
    }
    // Section startup replays the shared library unconditionally; a prompt
    // without one replays an empty compiled chunk instead, so the startup
    // sequence always has a program to replay.
    let shared = match promptforge_parser::detail::replay(&prompt) {
        Some(program) => program.clone(),
        None => crate::lua::LuaProgram::empty()?,
    };
    // Every run and every store call needs a declared store: the probe
    // stats the store root through a throwaway capability, so a handle
    // with no declaration fails here rather than on the first store call,
    // and a declared-but-failing backend's error fails the run.
    let probe = ctx
        .vfs
        .acquire(promptforge_vfs::Origin::new("store mount probe"))
        .map_err(Error::store)?;
    promptforge_vfs::detail::probe_store(&probe).map_err(Error::store)?;
    Ok(RunState::new(prompt, args, &ctx.vfs, shared, ctx))
}

#[cfg(test)]
mod tests;
