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

/// The outcome of one call to [`Run::step`]: the run either continues or
/// is over.
#[derive(Debug)]
pub enum Step {
    /// The run continues.
    ///
    /// `effects` lists the effects this step issued for the caller to
    /// perform, in issue order. Each comes with the provenance of the task
    /// that built it. An empty list means every chain is waiting on an
    /// effect that an earlier step issued. `events` lists the events the
    /// step reported, in order.
    Pending {
        /// The effects the caller must perform and answer through
        /// [`Run::resume`].
        effects: Vec<(EffectId, Provenance, Effect)>,
        /// The events the step reported.
        events: Vec<Event>,
    },
    /// The run is over.
    ///
    /// Carries the run's result and its last events. A step returns `Done`
    /// only after every issued effect has been answered. The event that
    /// marks the run's end comes with `Done`, or with an earlier `Pending`
    /// step when effects were still outstanding as the run ended.
    Done {
        /// The run's outcome.
        result: RunResult,
        /// The events reported since the previous step.
        events: Vec<Event>,
    },
}

/// One run of one prompt, which the caller drives through
/// [`step`](Self::step) and [`resume`](Self::resume).
///
/// `Run` is `Send`: one caller drives it at a time, and the thread may
/// change between calls. It holds its prompt through an `Arc`, so the
/// caller can parse a prompt once and run it many times.
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
    /// Builds a run of `prompt` with the arguments `args` and the context
    /// `ctx`.
    ///
    /// A context that skipped
    /// [`Environment::prepare`](super::Environment::prepare) runs with empty
    /// tool and model sets.
    ///
    /// A run that fails to start is still built, and its first `step`
    /// returns `Done` with the failure. A run fails to start when the
    /// prompt has no supported `promptforge:` version, when the context's
    /// filesystem declares no store, or when the store's backend fails its
    /// probe.
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

    /// Advances the run until it needs an answer, and returns the effects
    /// it issued and the events it reported.
    ///
    /// It returns [`Step::Pending`] while the run waits on an answer, and
    /// [`Step::Done`] once the run is over and every issued effect is
    /// answered. Calling `step` after `Done` is a caller error: it returns
    /// `Done` again with an internal failure.
    ///
    /// `step` cannot fail: a run's failures are values in
    /// [`RunResult::Failure`]. So the caller drives the loop and owns the
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

    /// Delivers the answer to one effect the run issued.
    ///
    /// The chain waiting on the effect resumes with the answer at the next
    /// `step`. For [`EffectAnswer::Dropped`], it resumes with a
    /// cancellation error. Any events the answer produces are reported by
    /// that next step.
    ///
    /// Each answer must match the kind of its effect: a chat answer for a
    /// `Chat` effect, a tool answer for a `ToolCall` effect, and so on. A
    /// mismatch is an internal error that ends the run. So is an answer for
    /// an id the run never issued, or a second answer for one effect. An
    /// answer for an effect whose chain stopped waiting is discarded.
    ///
    /// `resume` cannot fail: a bad answer ends the run with a
    /// [`RunResult::Failure`] instead of returning an error or panicking.
    pub fn resume(&mut self, id: EffectId, answer: EffectAnswer) {
        if let Some(scheduler) = self.scheduler.as_mut() {
            scheduler.resume(id, answer);
        }
    }

    /// Asks the run to stop by setting its cancel flag.
    ///
    /// Running Lua code sees the flag through its instruction hook. The
    /// next `step` tears down every chain. The run then reports itself as
    /// cancelled once every outstanding effect is answered.
    ///
    /// Cancellation is a request that later steps complete. The caller
    /// must answer each effect it abandons with [`EffectAnswer::Dropped`].
    /// Like `step` and `resume`, `cancel` cannot fail.
    pub fn cancel(&mut self) {
        self.cancel.cancel();
    }

    /// Returns the run's cancel flag, so the caller can cancel the run from
    /// another thread.
    #[must_use]
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }

    /// Returns whether the run's outcome is decided.
    ///
    /// The outcome is decided once the run has reported its end event, or
    /// when the run failed to start. Any effect still outstanding at that
    /// point is an orphan: only `Done` waits for its answer. The caller can
    /// read this after a `Pending` step to learn that it may drop what it
    /// holds for the run. This keeps control flow independent of the
    /// events, which are only a report.
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
