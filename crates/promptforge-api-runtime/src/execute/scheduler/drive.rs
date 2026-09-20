//! The run-level step: `resume -> match request -> dispatch -> resume with
//! answer`, run until no chain can proceed without a host answer. The step
//! starts the run's first chain on its first call, drains the ready queue,
//! and returns the effects the drain issued with the events it reported.
//! An empty ready queue with an empty pending table is a stall, which
//! fails loudly rather than hangs. The run's `Done` is withheld until
//! every issued effect has its answer, so no store op's access clone
//! outlives the run and every effect the host was handed has exactly one
//! answer.

use crate::execute::RunResult;
use crate::execute::error::RunError;
use crate::execute::run::{EffectAnswer, EffectId, Step};
use crate::execute::support::GENERIC_COMPLETION;
use crate::{Error, Result};
use promptforge_api_types::event::lifecycle;
use promptforge_api_types::ids::AbandonReason;

use super::{Phase, Scheduler};

impl Scheduler {
    /// Drains the ready queue and returns the step's outcome: the effects
    /// issued and the events reported while chains ran, or the run's
    /// result once it is over and every issued effect is answered. The
    /// first call starts the H1 pass when the prompt has H1 blocks, else
    /// the root chain over the prompt's sections.
    pub(crate) fn step(&mut self) -> Step {
        if matches!(self.phase, Phase::Fresh) {
            self.phase = Phase::Running;
            if let Err(error) = self.start() {
                self.end(Err(error));
            }
        }
        if matches!(self.phase, Phase::Running) {
            self.drain();
        }
        // Every unfinished chain is ready, pending on an effect, blocked on
        // a child, or waiting on a task, and a blocked or waiting chain
        // transitively bottoms out in a ready or pending chain, so an
        // empty ready queue with an empty pending table can only be a
        // scheduler bug (nothing ready, nothing pending, and whatever is
        // waiting can never be woken) - fail loudly rather than hang.
        if matches!(self.phase, Phase::Running) && self.ready.is_empty() && self.pending.is_empty()
        {
            self.end(Err(Error::internal(
                "the scheduler stalled with no ready chain and no in-flight request",
            )));
        }
        let effects = std::mem::take(&mut self.issued);
        let events = self.ctx.take_events();
        match &self.phase {
            Phase::Ending(_) if self.pending.is_empty() && self.orphaned.is_empty() => {
                let Phase::Ending(result) = std::mem::replace(&mut self.phase, Phase::Done) else {
                    unreachable!("the phase was matched as ending");
                };
                Step::Done {
                    result: match result {
                        Ok(text) => RunResult::Ok(text),
                        Err(Error::Interrupted) => RunResult::Cancelled,
                        Err(error) => RunResult::Failure(RunError::from(error)),
                    },
                    events,
                }
            }
            Phase::Done => Step::Done {
                result: RunResult::Failure(RunError::from(Error::internal(
                    "a finished run cannot be stepped again",
                ))),
                events,
            },
            Phase::Fresh | Phase::Running | Phase::Ending(_) => Step::Pending { effects, events },
        }
    }

    /// Applies one effect's answer. An answer for an orphaned effect (its
    /// chain stopped waiting) is discarded; an answer for an id the run
    /// never issued, or a second answer for one effect, ends the run with
    /// an internal error, as does a fatal outcome of the answer itself (a
    /// claims-model conflict). A run that has returned `Done` ignores
    /// every answer.
    pub(crate) fn resume(&mut self, id: EffectId, answer: EffectAnswer) {
        if matches!(self.phase, Phase::Done) {
            return;
        }
        if self.orphaned.remove(&id) {
            return;
        }
        // Every answer is applied here, on the caller's thread: the
        // round's events fire against the parked chain's own reporting
        // handles, a chat round's tool calls are checked against the scope
        // the chain advertised, a timer's firing wakes its waiter, and a
        // fatal store conflict ends the run.
        if let Err(error) = self.apply_answer(id, answer) {
            self.end(Err(error));
        }
    }

    /// Starts the run's first chain: the H1 pass when the prompt has H1
    /// blocks; an H1-less prompt goes straight to the walk, so its shared
    /// library never pays for a throwaway section-0 replay. A prompt with
    /// neither ends at once with the generic completion.
    fn start(&mut self) -> Result<()> {
        let prompt = self.prompt();
        if prompt.h1_blocks().is_empty() {
            if prompt.sections().is_empty() {
                self.end(Ok(GENERIC_COMPLETION.to_owned()));
                return Ok(());
            }
            self.start_root_walk()?;
        } else {
            let h1 = self.start_live_h1()?;
            self.ready.push_back(h1);
        }
        Ok(())
    }

    /// Runs every ready chain to its next suspension point. Cancellation
    /// is polled before each chain step: the instruction hook covers
    /// running Lua, and a host that cancels while every chain is
    /// suspended is observed on its next `step`.
    fn drain(&mut self) {
        let mut root_result = None;
        loop {
            if self.ctx.cancel().is_cancelled() {
                self.end(Err(Error::Interrupted));
                return;
            }
            let Some(id) = self.ready.pop_front() else {
                return;
            };
            if let Err(error) = self.step_chain(id, &mut root_result) {
                self.finish(id, Err(error), &mut root_result);
            }
            if let Some(result) = root_result.take() {
                self.end(result);
                return;
            }
        }
    }

    /// Decides the run: settles every live task exactly once (each
    /// reports `TaskAbandoned` - with `RunTerminated` for a task the run's
    /// end stranded directly, `OwnerAborted` for one nested under it and
    /// ended through `abort_subtree` - so a task stranded by a host cancel
    /// or a fatal answer keeps the one-terminal contract; a run that ended
    /// well has none left, its root chain having settled its own), tears
    /// every chain down (the suspended chains' frames
    /// drop unarmed - no `SECTION_FINISHED` - and every effect still out
    /// with the host becomes an orphan the host still answers), reports
    /// the run's end boundary after every task terminal, and holds
    /// `result` until the orphans are answered. A second decision keeps
    /// the first: the outcome that ended the run is the record.
    pub(super) fn end(&mut self, result: Result<String>) {
        if matches!(self.phase, Phase::Ending(_) | Phase::Done) {
            return;
        }
        self.settle_all_tasks(AbandonReason::RunTerminated);
        self.teardown();
        self.ctx.emitter().report(
            self.ctx.prompt().title(),
            if result.is_ok() {
                lifecycle::RUN_SUCCEEDED
            } else {
                lifecycle::RUN_FAILED
            },
        );
        self.phase = Phase::Ending(result);
    }

    /// Drops every chain's live state in the teardown order (the suspended
    /// coroutine, then the frame unarmed, then the access capability) and
    /// orphans every pending effect. The task slots keep their terminal
    /// state for inspection; every slot is terminal by now, `end` having
    /// settled the live ones, so nothing here reports.
    fn teardown(&mut self) {
        self.ready.clear();
        self.stack.clear();
        for chain in &mut self.chains {
            chain.coroutine = None;
            chain.incoming = None;
            chain.waiting_on.clear();
            chain.awaiting = None;
            chain.blocked = None;
            chain.frame = None;
            chain.access = None;
        }
        let pending: Vec<EffectId> = self.pending.drain().map(|(id, _)| id).collect();
        self.orphaned.extend(pending);
    }
}
