//! The chain lifecycle: arena insertion, and the two chain-end paths - a
//! chain's finish (the frame's teardown boundary, the chain-end rules for
//! the tasks it owns, and the outcome's delivery to the root, a call
//! parent, or a task slot) and the abort of a chain with everything it
//! transitively blocks on or owns.

use std::num::NonZeroUsize;

use promptforge_types::ids::{AbandonReason, ChainId, TaskId};

use crate::execute::context::RunState;
use crate::execute::protocol::Answer;
use crate::execute::run::EffectId;
use crate::execute::walk_target::SlicePath;
use crate::{Error, Result};

use super::chain_record::{BlockIndex, CallDepth, ChainKind, SectionIndex};
use super::{
    Chain, ChainIndex, ChatAnchor, Continuation, Counters, GENERIC_COMPLETION, Pending, Scheduler,
};

/// What one chain's step left the run with: the run goes on, or the step
/// decided the run's result.
#[derive(Debug)]
pub(super) enum ChainStep {
    /// The run goes on.
    Continue,
    /// The run ended with this result: the root's outcome, an H1
    /// short-circuit, or a failure fatal to the whole run.
    RunEnded(Result<String>),
}

/// Everything a new chain starts from.
pub(super) struct ChainStart {
    /// The chain's hierarchical id.
    pub(super) lineage: ChainId,
    /// The id counters the chain continues from: zero for a fresh chain,
    /// the H1 pass's for the root walk that follows it.
    pub(super) counters: Counters,
    /// The chain's fork of the run context.
    pub(super) ctx: RunState,
    /// The slice the chain walks.
    pub(super) slice: SlicePath,
    /// The section of `slice` the chain starts at.
    pub(super) index: SectionIndex,
    /// What kind of chain it is.
    pub(super) kind: ChainKind,
    /// The `var` the chain's first section seeds from: a call or task
    /// chain's caller snapshot, discarded with the chain.
    pub(super) var: serde_json::Value,
    /// The chain's call nesting depth.
    pub(super) call_depth: CallDepth,
    /// The chain's effective admission limit for the tasks it spawns:
    /// the caller's own for a call child or a task, the run's ceiling for
    /// the root.
    pub(super) concurrency: NonZeroUsize,
}

impl Scheduler {
    /// Allocates the next child id under `owner`'s chain: the owner's id
    /// extended by its local child counter, which `call` children and
    /// spawned tasks share, so the ids a chain hands out depend only on
    /// the order of its own dispatches.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when one chain has started `u32::MAX`
    /// children, which no reachable run does.
    pub(super) fn allocate_child_id(&mut self, owner: ChainIndex) -> Result<ChainId> {
        let chain = &mut self.chains[owner.index()];
        let index = chain.counters.next_child;
        chain.counters.next_child = index
            .checked_add(1)
            .ok_or(Error::internal("a chain's child count cannot exceed u32"))?;
        Ok(chain.lineage.child(index))
    }

    /// Creates one chain from `start` and returns its arena index. A
    /// fresh chain enters its first section on its first step, taking
    /// entry 0 of its own id; the root walk continues the counters of the
    /// H1 pass it follows. Until its first entry the chain reports under
    /// the section at its start index, or the prompt's title when the
    /// index is past the slice. The chain's task is its own id for a task
    /// chain, its call parent's for a `call` child, and task `0` for the
    /// root walk and the H1 pass.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the run's chain count exceeds `u32`.
    pub(super) fn start_chain(&mut self, start: ChainStart) -> Result<ChainIndex> {
        if self.chains.len() >= self.max_chains {
            return Err(Error::internal("a run's chain count cannot exceed u32"));
        }
        let id = ChainIndex(u32::try_from(self.chains.len()).map_err(|error| {
            Error::internal_with_cause("a run's chain count cannot exceed u32", error)
        })?);
        let task = match &start.kind {
            ChainKind::Root | ChainKind::H1 => TaskId::from(ChainId::root()),
            ChainKind::Call { parent, .. } => self.chains[parent.index()].task.clone(),
            ChainKind::Task { .. } => TaskId::from(start.lineage.clone()),
        };
        let entered = start
            .slice
            .name_at(start.ctx.prompt(), start.index.get())
            .to_owned();
        self.chains.push(Chain {
            lineage: start.lineage,
            counters: start.counters,
            kind: start.kind,
            task,
            seed: None,
            waiting_on: Vec::new(),
            awaiting: None,
            batch: Vec::new(),
            blocked: None,
            task_notices: Vec::new(),
            note: None,
            ctx: start.ctx,
            access: None,
            frame: None,
            slice: start.slice,
            index: start.index,
            entered,
            positions: Vec::new(),
            block: BlockIndex::FIRST,
            coroutine: None,
            incoming: None,
            pending_prose: None,
            var: start.var,
            call_depth: start.call_depth,
            concurrency: start.concurrency,
            advertised: None,
            anchor: ChatAnchor::default(),
        });
        Ok(id)
    }

    /// Finishes one chain: the frame's teardown boundary when the chain
    /// ends mid-section, the chain-end rules for the tasks it owns (a live
    /// author task makes the outcome `tasks_live`; every live task is
    /// abandoned), then the chain-end joins - every owned task's final
    /// clock merges into this chain's own, so a waiting owner sees the
    /// whole subtree transitively - and finally the outcome's delivery:
    /// the run's result for the root chain, the call answer for a child
    /// chain, the task slot's outcome for a spawned chain.
    ///
    /// `outcome` is the chain's end: a scalar return's value, `None` for a
    /// walk that ran off its slice's last section, or the chain's failure.
    pub(super) fn finish(&mut self, id: ChainIndex, outcome: Result<Option<String>>) -> ChainStep {
        let chain = &mut self.chains[id.index()];
        let parent = chain.kind.parent();
        let is_task = chain.kind.owner().is_some();
        // `None` when the chain ended by exhausting its slice: the last
        // section's frame already dropped at the fall-through.
        let mut frame = chain.frame.take();
        // Taken now, dropped last: the VM's store closures hold their own
        // Arc clones of the capability, so the identity's last reference
        // drops only when both are gone. The chain-end joins run while
        // this clone is still held.
        let access = chain.access.take();
        // The live H1 pass never arms completion: SECTION_FINISHED is a
        // walked section's boundary, not the setup pass's. Its completion
        // paths (fall-through, scalar return) handle the frame themselves;
        // this guard keeps an H1 frame that reaches here - an error path -
        // unarmed.
        let is_h1 = chain.kind.is_h1();
        let outcome = outcome.and_then(|returned| {
            // A chain ending mid-section (a scalar return) reads its final
            // var back before teardown, exactly as a completed section does
            // at fall-through (the walk rolls it forward; a call chain
            // or a task chain discards its clone), and arms the completion
            // flag so the frame's drop fires SECTION_FINISHED. A failure -
            // the read-back's included - drops the frame unarmed.
            if let Some(frame) = frame.as_mut() {
                frame.read_var()?;
                if !is_h1 {
                    frame.mark_completed();
                }
            }
            let text = match returned {
                Some(value) => value,
                // A walk that ran off its slice produced no scalar result:
                // the top-level chain falls back to the shared generic
                // completion; a call chain or a task chain to the empty
                // string.
                None if parent.is_none() && !is_task => GENERIC_COMPLETION.to_owned(),
                None => String::new(),
            };
            Ok(text)
        });
        // The frame drops here: the single teardown boundary.
        drop(frame);
        // The chain's tasks end with it: a live author task turns a
        // success into `tasks_live`, and every live task is abandoned.
        let outcome = self.settle_owned_tasks(id, outcome);
        // The chain-end joins run after the settle - the abandoned tasks'
        // identities ended there, so their final clocks are recorded -
        // and before the access drops, so this chain's final clock
        // transitively covers everything its tasks did.
        if let Some(access) = &access {
            self.join_owned_tasks(id, access);
        }
        drop(access);
        // A task chain that was holding admission slots releases them at
        // its end, before its owner is woken: the owner's re-acquire and
        // any queued sibling need the freed capacity.
        self.release_chain_slots(id);
        // A determinism violation is fatal to the run on the spot and
        // never resumes into Lua as a catchable member failure: a shim
        // that re-raises it after its own cleanup yields would flatten
        // the typed error, so the outcome ends the run here, exactly as
        // the answer boundary does.
        let outcome = match outcome {
            Err(error @ Error::Determinism(_)) => return ChainStep::RunEnded(Err(error)),
            outcome => outcome,
        };
        if is_task {
            // A missing slot is a scheduler bug: fail the run loudly rather
            // than lose the task's outcome.
            return match self.complete_task(id, outcome) {
                Ok(()) => ChainStep::Continue,
                Err(error) => ChainStep::RunEnded(Err(error)),
            };
        }
        match parent {
            None => ChainStep::RunEnded(outcome),
            Some(parent_id) => {
                self.answer_inline(parent_id, Answer::Call(outcome));
                ChainStep::Continue
            }
        }
    }

    /// Aborts one chain and everything it transitively blocks on or owns -
    /// its call children and the tasks it spawned (a fanout's arms among
    /// them; each abandoned as `owner_aborted`, its own subtree aborted in
    /// turn): the chain leaves the ready queue and the pending table, its
    /// in-flight leaf effects are orphaned, and its state drops in the
    /// teardown order (the suspended coroutine, then the frame unarmed - no
    /// `SECTION_FINISHED`). The chain's own task slot, if it is a task, is
    /// the caller's to settle: the owner's chain end abandons it, a cancel
    /// arm cancels it.
    pub(super) fn abort_subtree(&mut self, id: ChainIndex) {
        // The arena is u32-bounded at insertion (`start_chain`), so the
        // index range covers every chain.
        let children: Vec<ChainIndex> = (0..)
            .map(ChainIndex)
            .zip(&self.chains)
            .filter(|(_, chain)| chain.kind.parent() == Some(id))
            .map(|(child, _)| child)
            .collect();
        for child in children {
            self.abort_subtree(child);
        }
        // The tasks this chain owns end with it; their outcomes have no
        // one to reach, so the leaked-author list is moot here.
        self.abandon_owned_tasks(id, AbandonReason::OwnerAborted);
        self.ready.retain(|ready| *ready != id);
        // The chain's own parked leaf effects: one, or every call of a
        // `tool_calls` batch. A timer the chain owned is keyed under it
        // too, but `abandon_owned_tasks` above already aborted every live
        // one. Each tool call's identity ends as it is orphaned, so the
        // entries go in issue order rather than the table's.
        let mut effects: Vec<EffectId> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.chain == id)
            .map(|(effect, _)| *effect)
            .collect();
        effects.sort_unstable();
        for effect in effects {
            self.abort_effect(effect);
        }
        let access = {
            let chain = &mut self.chains[id.index()];
            chain.coroutine = None;
            chain.incoming = None;
            // A chain aborted mid-wait leaves its set: no member's end may
            // wake a dead chain. The model's parked `await_tasks` goes
            // with it; its timer was abandoned above with the chain's
            // other owned slots.
            chain.waiting_on.clear();
            chain.awaiting = None;
            chain.batch.clear();
            chain.blocked = None;
            chain.frame = None;
            chain.access.take()
        };
        // A queued chain aborted before admission leaves both queues, and
        // a running task's held slots are given back.
        self.resuming.retain(|queued| *queued != id);
        self.spawned.retain(|queued| *queued != id);
        self.release_chain_slots(id);
        // The chain-end join: the aborted chain's final clock still
        // transitively covers its tasks, for whoever joins the aborted
        // chain itself.
        if let Some(access) = &access {
            self.join_owned_tasks(id, access);
        }
    }

    /// Orphans one in-flight leaf effect whose chain is going away: the
    /// pending entry leaves, and the id is recorded so the caller's answer,
    /// when it arrives, is discarded rather than failing the run. A tool
    /// call's identity ends here as its answer would end it, so what the
    /// tool already did is joined into the parked chain and anything it
    /// tries later is refused. A tool call is also listed as withdrawn,
    /// for the step to report. The caller still owes the answer: `Done`
    /// waits for every issued effect, so the run ends only once the
    /// caller has answered all of them.
    pub(super) fn abort_effect(&mut self, effect: EffectId) {
        if let Some(Pending {
            chain,
            resume: Continuation::ToolCall(call),
        }) = self.pending.remove(&effect)
        {
            self.end_tool_call(chain, call.exec);
            self.withdrawn.push(effect);
        }
        self.orphaned.insert(effect);
    }
}
