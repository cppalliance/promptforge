//! The chain lifecycle: arena insertion, and the two chain-end paths - a
//! chain's finish (the frame's teardown boundary, the chain-end rules for
//! the tasks it owns, and the outcome's delivery to the root, a call
//! parent, a fanout join, or a task slot) and the abort of a chain with
//! everything it transitively blocks on or owns.

use promptforge_api_types::ids::{AbandonReason, ChainId, TaskId};

use crate::execute::context::RunState;
use crate::execute::protocol::Answer;
use crate::execute::support::GENERIC_COMPLETION;
use crate::parser::Section;
use crate::{Error, Result};

use super::joins::{ArmState, FanoutId};
use super::{Chain, ChainIndex, Counters, RequestId, Scheduler};

impl<'a> Scheduler<'a> {
    /// Allocates the next child id under `owner`'s chain: the owner's id
    /// extended by its local child counter, which `call` children and
    /// spawned arms share, so the ids a chain hands out depend only on
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

    /// Creates one chain over `slice` from `index` under the hierarchical
    /// `lineage` with its id `counters` and returns its arena index. A
    /// fresh chain starts its counters at zero and enters its first
    /// section on its first step, taking entry 0 of its own id; the root
    /// walk continues the counters of the H1 pass it follows. The chain's
    /// `var` slot seeds from `var` (a call chain's or arm's caller
    /// snapshot, discarded with the chain). `arm` carries the fanout-arm
    /// state for an arm chain. The chain's task is its call parent's when
    /// it has one, else task `0`; a spawned chain's dispatch overwrites it
    /// with the chain's own id, and an arm's with its caller's.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the run's chain count exceeds `u32`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the chain keeps its lineage, counters, context fork, position, parent, var seed, depth, and arm state explicit and linear"
    )]
    pub(super) fn start_chain(
        &mut self,
        lineage: ChainId,
        counters: Counters,
        ctx: RunState,
        slice: &'a [Section],
        index: usize,
        parent: Option<ChainIndex>,
        var: &serde_json::Value,
        call_depth: usize,
        arm: Option<ArmState<'a>>,
    ) -> Result<ChainIndex> {
        if self.chains.len() >= self.max_chains {
            return Err(Error::internal("a run's chain count cannot exceed u32"));
        }
        let id = ChainIndex(
            u32::try_from(self.chains.len())
                .map_err(|_| Error::internal("a run's chain count cannot exceed u32"))?,
        );
        let task = parent.map_or_else(
            || TaskId::from(ChainId::root()),
            |parent| self.chains[parent.index()].task.clone(),
        );
        self.chains.push(Chain {
            lineage,
            counters,
            task,
            owner: None,
            seed: None,
            waiting_on: Vec::new(),
            blocked: None,
            task_notices: Vec::new(),
            note: None,
            ctx,
            access: None,
            frame: None,
            slice,
            index,
            positions: Vec::new(),
            block: 0,
            coroutine: None,
            incoming: None,
            pending_prose: None,
            var: var.clone(),
            call_depth,
            client: None,
            parent,
            advertised: None,
            arm,
            h1: None,
        });
        Ok(id)
    }

    /// Finishes one chain: the frame's teardown boundary when the chain
    /// ends mid-section, the chain-end rules for the tasks it owns (a live
    /// author task makes the outcome `tasks_live`; every live task is
    /// abandoned), then the outcome's delivery - the run's result for
    /// the root chain, the call answer for a child chain, the join
    /// slot's result for a fanout arm, the task slot's outcome for a
    /// spawned chain.
    ///
    /// `outcome` is the chain's end: a scalar return's value, `None` for a
    /// walk that ran off its slice's last section, or the chain's failure.
    pub(super) fn finish(
        &mut self,
        id: ChainIndex,
        outcome: Result<Option<String>>,
        root_result: &mut Option<Result<String>>,
    ) {
        let chain = &mut self.chains[id.index()];
        let parent = chain.parent;
        let is_task = chain.owner.is_some();
        let arm = chain.arm.take();
        // `None` when the chain ended by exhausting its slice: the last
        // section's frame already dropped at the fall-through.
        let mut frame = chain.frame.take();
        // Taken now, dropped after the frame: the VM's store closures hold
        // their own Arc clones of the capability, so the identity's claims
        // release only when both are gone - at chain end, before a fanout
        // join resumes the parent into its merge. A call chain's slot is a
        // borrowed clone, so its drop never releases the parent's identity.
        let access = chain.access.take();
        // The live H1 pass never arms completion: SECTION_FINISHED is a
        // walked section's boundary, not the setup pass's. Its completion
        // paths (fall-through, scalar return) handle the frame themselves;
        // this guard keeps an H1 frame that reaches here - an error path -
        // unarmed.
        let is_h1 = chain.h1.is_some();
        let outcome = outcome.and_then(|returned| {
            // A chain ending mid-section (a scalar return) reads its final
            // var back before teardown, exactly as a completed section does
            // at fall-through (the walk rolls it forward; a call chain
            // or a fanout arm discards its clone), and arms the completion
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
                // completion; a call chain, a fanout arm, or a task chain
                // to the empty string.
                None if parent.is_none() && arm.is_none() && !is_task => {
                    GENERIC_COMPLETION.to_owned()
                }
                None => String::new(),
            };
            Ok(text)
        });
        // The frame drops here: the single teardown boundary.
        drop(frame);
        drop(access);
        // The chain's tasks end with it: a live author task turns a
        // success into `tasks_live`, and every live task is abandoned.
        let outcome = self.settle_owned_tasks(id, outcome);
        if let Some(arm) = arm {
            self.complete_arm(arm, outcome);
            return;
        }
        if is_task {
            // A missing slot is a scheduler bug: fail the run loudly rather
            // than lose the task's outcome.
            if let Err(error) = self.complete_task(id, outcome) {
                *root_result = Some(Err(error));
            }
            return;
        }
        match parent {
            None => *root_result = Some(outcome),
            Some(parent_id) => {
                debug_assert_eq!(
                    self.stack.pop(),
                    Some(id),
                    "a finishing child chain is the call stack's top"
                );
                self.chains[parent_id.index()].incoming = Some(Answer::Call(outcome));
                self.ready.push_back(parent_id);
            }
        }
    }

    /// Aborts one chain and everything it transitively blocks on or owns -
    /// its call children, the arms of its nested fanouts, and the tasks it
    /// spawned (each abandoned as `owner_aborted`, its own subtree aborted
    /// in turn) - the scheduler port of dropping a spawned arm task: the
    /// chain leaves the ready queue and the pending table, its in-flight
    /// leaf I/O task is aborted, and its state drops in the teardown order
    /// (the suspended coroutine, then the frame unarmed - no
    /// `SECTION_FINISHED` - then the arm state, whose finalizer drop
    /// reports `FANOUT_ARM_CANCELLED`). The chain's own task slot, if it
    /// is a task, is the caller's to settle: the owner's chain end
    /// abandons it, a cancel arm cancels it.
    pub(super) fn abort_subtree(&mut self, id: ChainIndex) {
        // Nested fanouts this chain parents: their arms abort with it, and
        // the removed join has no answer to deliver - the parent is dead.
        let nested: Vec<FanoutId> = self
            .joins
            .iter()
            .filter(|(_, join)| join.parent == id)
            .map(|(fanout, _)| *fanout)
            .collect();
        for fanout in nested {
            self.joins.remove(&fanout);
            for arm in self.arm_chains_of(fanout) {
                self.abort_subtree(arm);
            }
        }
        // The arena is u32-bounded at insertion (`start_chain`), so the
        // index conversion cannot fail.
        let children: Vec<ChainIndex> = self
            .chains
            .iter()
            .enumerate()
            .filter(|(_, chain)| chain.parent == Some(id))
            .filter_map(|(index, _)| u32::try_from(index).ok().map(ChainIndex))
            .collect();
        for child in children {
            self.abort_subtree(child);
        }
        // The tasks this chain owns end with it; their outcomes have no
        // one to reach, so the leaked-author list is moot here.
        self.abandon_owned_tasks(id, AbandonReason::OwnerAborted);
        self.ready.retain(|ready| *ready != id);
        // The chain's own parked leaf request. A timer the chain owned is
        // keyed under it too, but `abandon_owned_tasks` above already
        // aborted every live one, so this is the only entry left.
        let request = self
            .pending
            .iter()
            .find_map(|(request, chain)| (*chain == id).then_some(*request));
        if let Some(request) = request {
            self.abort_request(request);
        }
        // A chain on the call stack is the top here: only its own
        // descendants sit above it, and the recursion already removed them.
        if self.stack.last() == Some(&id) {
            self.stack.pop();
        }
        let chain = &mut self.chains[id.index()];
        chain.coroutine = None;
        chain.incoming = None;
        // A chain aborted mid-wait leaves its set: no member's end may wake
        // a dead chain.
        chain.waiting_on.clear();
        chain.blocked = None;
        chain.frame = None;
        chain.access = None;
        chain.arm = None;
    }

    /// Drops one in-flight leaf request whose chain is going away: the
    /// pending entry leaves, the id is recorded as aborted, and the leaf
    /// task is aborted.
    pub(super) fn abort_request(&mut self, request: RequestId) {
        self.pending.remove(&request);
        // Record the aborted request so its task's late answer (a send
        // that landed before the abort) is the one unknown-id answer
        // the driver discards; anything else stays a loud invariant
        // failure.
        self.aborted_requests.insert(request);
        // The handle stays in `io_tasks`: aborting a blocking-pool op
        // detaches rather than interrupts, so the op's access clone -
        // and the claims it holds - releases only when the op finishes.
        // The run-end drain awaits the handle, keeping claim release
        // bounded to the run's lifetime on this path too; if the op's
        // late answer arrives first, the answer loop takes the handle.
        if let Some(task) = self.io_tasks.get(&request) {
            task.abort();
        }
    }
}
