//! The chain lifecycle: arena insertion, and the two chain-end paths - a
//! chain's finish (the frame's teardown boundary and the outcome's
//! delivery to the root, a call parent, or a fanout join) and the abort of
//! a chain with everything it transitively blocks on.

use crate::execute::context::RunState;
use crate::execute::protocol::Answer;
use crate::execute::support::GENERIC_COMPLETION;
use crate::parser::Section;
use crate::{Error, Result};

use super::tasks::{ArmState, FanoutId};
use super::{Chain, ChainId, Scheduler};

impl<'a> Scheduler<'a> {
    /// Creates one chain over `slice` from `index` and returns its id. The
    /// chain enters its first section on its first step. The chain's
    /// `var` slot seeds from `var` (a call chain's or arm's caller
    /// snapshot, discarded with the chain). `arm` carries the fanout-arm
    /// state for an arm chain.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the run's chain count exceeds `u32`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the chain keeps its context fork, position, parent, var seed, depth, and arm state explicit and linear"
    )]
    pub(super) fn start_chain(
        &mut self,
        ctx: RunState,
        slice: &'a [Section],
        index: usize,
        parent: Option<ChainId>,
        var: &serde_json::Value,
        call_depth: usize,
        arm: Option<ArmState<'a>>,
    ) -> Result<ChainId> {
        if self.chains.len() >= self.max_chains {
            return Err(Error::internal("a run's chain count cannot exceed u32"));
        }
        let id = ChainId(
            u32::try_from(self.chains.len())
                .map_err(|_| Error::internal("a run's chain count cannot exceed u32"))?,
        );
        self.chains.push(Chain {
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
            arm,
            h1: None,
        });
        Ok(id)
    }

    /// Finishes one chain: the frame's teardown boundary when the chain
    /// ends mid-section, then the outcome's delivery - the run's result for
    /// the root chain, the call answer for a child chain, the join
    /// slot's result for a fanout arm.
    ///
    /// `outcome` is the chain's end: a scalar return's value, `None` for a
    /// walk that ran off its slice's last section, or the chain's failure.
    pub(super) fn finish(
        &mut self,
        id: ChainId,
        outcome: Result<Option<String>>,
        root_result: &mut Option<Result<String>>,
    ) {
        let chain = &mut self.chains[id.index()];
        let parent = chain.parent;
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
                // completion; a call chain or a fanout arm to the empty
                // string.
                None if parent.is_none() && arm.is_none() => GENERIC_COMPLETION.to_owned(),
                None => String::new(),
            };
            Ok(text)
        });
        // The frame drops here: the single teardown boundary.
        drop(frame);
        drop(access);
        if let Some(arm) = arm {
            self.complete_arm(arm, outcome);
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

    /// Aborts one chain and everything it transitively blocks on - its
    /// call children and the arms of its nested fanouts - the scheduler
    /// port of dropping a spawned arm task: the chain leaves the ready
    /// queue and the pending table, its in-flight leaf I/O task is aborted,
    /// and its state drops in the teardown order (the suspended coroutine,
    /// then the frame unarmed - no `SECTION_FINISHED` - then the arm state,
    /// whose finalizer drop reports `FANOUT_ARM_CANCELLED`).
    pub(super) fn abort_subtree(&mut self, id: ChainId) {
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
        let children: Vec<ChainId> = self
            .chains
            .iter()
            .enumerate()
            .filter(|(_, chain)| chain.parent == Some(id))
            .filter_map(|(index, _)| u32::try_from(index).ok().map(ChainId))
            .collect();
        for child in children {
            self.abort_subtree(child);
        }
        self.ready.retain(|ready| *ready != id);
        let request = self
            .pending
            .iter()
            .find_map(|(request, chain)| (*chain == id).then_some(*request));
        if let Some(request) = request {
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
        // A chain on the call stack is the top here: only its own
        // descendants sit above it, and the recursion already removed them.
        if self.stack.last() == Some(&id) {
            self.stack.pop();
        }
        let chain = &mut self.chains[id.index()];
        chain.coroutine = None;
        chain.incoming = None;
        chain.frame = None;
        chain.access = None;
        chain.arm = None;
    }
}
