//! The run: the engine's host boundary, four methods exchanging effects and
//! events as values.
//!
//! A [`Run`] is a deterministic state machine over one prompt. The host
//! calls [`step`](Run::step), which drains every chain that can make
//! progress and returns the leaf [`Effect`]s those chains issued (each
//! stamped with the [`Provenance`] of the task that built it) beside the
//! [`Event`]s the step reported; the host performs the effects however it
//! likes and hands each answer back through [`resume`](Run::resume), one
//! per arriving answer, then steps again. The run performs no I/O, reads
//! no clock, and holds no host trait objects: given the same context and
//! the same answers it issues the same effects, events, and ids.
//!
//! [`Step::Done`] is withheld while any issued effect is unanswered, so a
//! host that has answered every effect it was handed - a drop counts - can
//! rely on the run's end being the end of every effect too. An effect a
//! chain stopped waiting for (its task was cancelled or abandoned) still
//! wants its one answer; the run discards it on arrival.
//!
//! [`cancel`](Run::cancel) sets the run's synchronous flag. The Lua
//! instruction hook polls it, so a running chunk aborts promptly; the next
//! `step` tears every chain down and reports the run as cancelled once the
//! outstanding effects are answered - a host cancelling a run answers each
//! effect it abandons with [`EffectAnswer::Dropped`].
//!
//! The effect vocabulary itself - [`Effect`], its serializable
//! [`EffectRecord`], [`EffectAnswer`], and [`EffectId`] - lives in the
//! `effect` child module and is re-exported here.

use std::sync::Arc;

use promptforge_api_types::event::Event;
use promptforge_api_types::ids::Provenance;

#[path = "run-effect.rs"]
mod effect;

pub use effect::{
    AnswerRecord, ChatAnswerRecord, Effect, EffectAnswer, EffectId, EffectRecord,
    InputAnswerRecord, StoreAnswerRecord, ToolAnswerRecord,
};

use crate::cancel::CancelHandle;
use crate::parser::{ParseErrorKind, Prompt};
use crate::store::VfsRef;
use crate::{Error, Result};

use super::RunResult;
use super::config::RunContext;
use super::context::RunState;
use super::error::RunError;
use super::scheduler::Scheduler;

/// What one [`Run::step`] produced.
#[derive(Debug)]
pub enum Step {
    /// The run is not over. `effects` are the leaf effects this step
    /// issued, in issue order, each with the provenance of the task that
    /// built it; an empty list means every chain waits on an effect
    /// already issued. `events` are the reports the step made, in order.
    Pending {
        /// The effects the host performs and answers through
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

/// One run of one prompt, driven by a host through
/// [`step`](Self::step) and [`resume`](Self::resume).
///
/// `Run` is `Send`: one caller drives it at a time, and the thread may
/// change between calls. It owns its prompt through an `Arc`, so the host
/// keeps parsing once and running many times.
///
/// # Examples
/// A prompt whose only section returns a literal issues no effect, so a
/// host drives it to `Done` in one step:
/// ```
/// use std::sync::Arc;
///
/// use promptforge_api_runtime::execute::{Run, RunContext, RunResult, Step};
/// use promptforge_api_runtime::parser::Prompt;
/// use promptforge_api_types::timestamp::Timestamp;
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
    /// supported `promptforge:` version, or a store handle whose mounted
    /// backend fails, yields a run whose first `step` is `Done` with the
    /// failure.
    #[must_use]
    pub fn new(prompt: Arc<Prompt>, args: &str, ctx: RunContext) -> Run {
        // The context's one flag, held here so a stillborn run still has
        // the handle `cancel` and `cancel_handle` name.
        let cancel = ctx.cancel.clone();
        match prepare_state(prompt, args, ctx) {
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
    /// answered. A step after `Done` is a host error and reports an
    /// internal failure.
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
    /// that step. An answer for an effect whose chain stopped waiting is
    /// discarded. An answer for an id the run never issued, or a second
    /// answer for one effect, is an internal error that ends the run.
    pub fn resume(&mut self, id: EffectId, answer: EffectAnswer) {
        if let Some(scheduler) = self.scheduler.as_mut() {
            scheduler.resume(id, answer);
        }
    }

    /// Sets the run's cancel flag. Running Lua observes it from its
    /// instruction hook; the next `step` tears every chain down and, once
    /// the outstanding effects are answered, reports the run as cancelled.
    pub fn cancel(&mut self) {
        self.cancel.cancel();
    }

    /// The run's cancel flag, for a host that cancels from another thread.
    #[must_use]
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }

    /// Whether the run's outcome is decided: its end boundary has been
    /// reported (or it never started) and every effect still out is an
    /// orphan whose answer only `Done` waits on. A host reads this after
    /// a `Pending` step to learn it may drop what it holds, so control
    /// never rides on the events, which are a report and not a decision.
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

/// Assembles the run state from the host's context: the version gate, the
/// shared library, and the store mount, in the order the run has always
/// checked them.
///
/// # Errors
/// Returns [`Error::UnsupportedVersion`] or a structural parse error for a
/// prompt that is not a supported promptforge prompt, the Lua error when
/// the empty shared chunk cannot compile, or [`Error::Store`] when the
/// mounted store backend fails the mount probe.
fn prepare_state(prompt: Arc<Prompt>, args: &str, mut ctx: RunContext) -> Result<RunState> {
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
    // sequence carries no `Option` branch.
    let shared = match prompt.replay() {
        Some(program) => program.clone(),
        None => crate::lua::LuaProgram::empty()?,
    };
    // The stock handle carries the store mount; a hand-built router lacking
    // it gets a fresh memory store overlaid as a defensive fallback, so a
    // run never fails for want of the mount. A mounted-but-failing backend
    // is never shadowed by the throwaway overlay: its error fails the run.
    if !store_mount_present(&ctx.vfs).map_err(Error::Store)? {
        ctx.vfs = ctx.vfs.overlay(
            promptforge_vfs::STORE_MOUNT,
            shared_vfs::MemoryBackend::new(),
        );
    }
    Ok(RunState::new(prompt, args, &ctx.vfs, shared, &ctx))
}

/// Whether the handle already serves the store mount. The probe stats the
/// mount root through a throwaway capability: a mounted backend answers
/// (the memory backend's root always exists), an unmounted path is
/// `NotFound`. Only `NotFound` means "mount absent": any other error is the
/// mounted backend's own failure and propagates, so a loud backend failure
/// is never converted into the run silently reading and writing a
/// throwaway overlay. The probe's identity and claim release with the
/// access.
fn store_mount_present(vfs: &VfsRef) -> std::result::Result<bool, shared_vfs::VfsError> {
    match vfs
        .acquire(shared_vfs::Origin::new("store mount probe"))?
        .stat(promptforge_vfs::STORE_MOUNT)
    {
        Ok(_) => Ok(true),
        Err(shared_vfs::VfsError::NotFound(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "run-tests.rs"]
mod tests;
