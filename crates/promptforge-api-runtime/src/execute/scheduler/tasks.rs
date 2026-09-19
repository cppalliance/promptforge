//! The task arena and the `spawn` arm. A task is a chain the scheduler
//! runs beside its spawner instead of in place of it: `tasks.spawn` starts
//! the chain over the target's slice - under `call`'s target resolution
//! and depth cap and `fanout`'s worker validation - registers a slot for it
//! keyed by the chain's own hierarchical id, and resumes the spawner at
//! once with the id. The spawner runs first; the child runs when the
//! spawner suspends or ends, exactly as any ready chain does.
//!
//! The slot outlives the chain. When the chain ends, its outcome lands in
//! the slot and the slot moves to `Done`; the owner later takes the result
//! through a wait, which moves it to `Delivered`. Cancellation and
//! abandonment (the owner ending first) are the two other terminal states,
//! kept apart because the log and the model notice must tell "stopped on
//! purpose" from "lost its owner".

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use promptforge_api_types::ids::{TaskId, TaskOrigin};

use crate::execute::protocol::Answer;
use crate::execute::section_context::TaskSeed;
use crate::execute::support::MAX_CALL_DEPTH;
use crate::observe::Observation;
use crate::{Error, Result};

use super::{ChainIndex, Counters, RequestId, Scheduler, prompt_origin};

/// Where a task's work runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TaskBacking {
    /// A chain in the arena: every author- or model-started task.
    Chain(ChainIndex),
    /// An in-flight leaf request: the internal timer behind a wait's
    /// timeout, never author-visible.
    #[expect(dead_code, reason = "constructed by the timer arm of a later step")]
    Effect(RequestId),
}

/// One task's lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskState {
    /// The backing chain or request is live.
    Running,
    /// The backing chain ended; its outcome waits in the slot.
    Done,
    /// The owner took the outcome through a wait.
    #[expect(dead_code, reason = "reached by the wait arms of the next step")]
    Delivered,
    /// Someone cancelled the task.
    #[expect(dead_code, reason = "reached by the cancel arm of the next step")]
    Cancelled,
    /// The owner ended while the task was live, so the engine ended it.
    #[expect(dead_code, reason = "reached by the chain-end rules of the next step")]
    Abandoned,
}

/// One task's slot in the arena.
#[derive(Debug)]
pub(super) struct TaskSlot {
    /// Where the task's work runs.
    #[expect(
        dead_code,
        reason = "read by the wait and status arms of the next step"
    )]
    pub(super) backing: TaskBacking,
    /// The chain that started the task: the only chain allowed to wait on,
    /// inspect, or cancel it.
    #[expect(dead_code, reason = "checked by the owner-only rule of the next step")]
    pub(super) owner: ChainIndex,
    /// The principal that started the task.
    #[expect(dead_code, reason = "read by the chain-end rules of the next step")]
    pub(super) origin: TaskOrigin,
    /// The name of the section the task's chain started at: the section
    /// its terminal observations report under.
    pub(super) target: String,
    /// The task's lifecycle state.
    pub(super) state: TaskState,
    /// The chain's final text or failure, held from the chain's end until
    /// the owner takes it.
    pub(super) outcome: Option<Result<String>>,
}

impl Scheduler<'_> {
    /// Dispatches a `spawn` request: constructs the task's chain, registers
    /// its slot, and resumes the spawner with the task's id; the child is
    /// enqueued behind the spawner, so `spawn` returns before the child
    /// runs. Every dispatch failure - the depth cap, target resolution, the
    /// worker check, chain construction - is the call's answer, resumed
    /// into the spawner so an author `pcall` can catch it.
    pub(super) fn dispatch_spawn(
        &mut self,
        id: ChainIndex,
        target: &str,
        input: Option<&str>,
        seed: TaskSeed,
        var: &serde_json::Value,
        origin: TaskOrigin,
    ) {
        match self.prepare_spawn(id, target, input, seed, var, origin) {
            Ok((task, child)) => {
                self.chains[id.index()].incoming = Some(Answer::Spawn(Ok(task)));
                self.ready.push_back(id);
                self.ready.push_back(child);
            }
            Err(error) => {
                self.chains[id.index()].incoming = Some(Answer::Spawn(Err(error)));
                self.ready.push_back(id);
            }
        }
    }

    /// The fallible half of spawn dispatch: `call`'s depth cap against the
    /// spawner's call-depth field, `call`'s target resolution over the
    /// spawner's visible set, `fanout`'s worker validation, then the task
    /// chain one level deeper under the spawn's `args` and `var` snapshot,
    /// with its own access capability (a concurrent thread of execution
    /// under the claims model, spawned from the spawner's so the spawn is
    /// the happens-before edge) and a fresh turn counter.
    fn prepare_spawn(
        &mut self,
        id: ChainIndex,
        target: &str,
        input: Option<&str>,
        seed: TaskSeed,
        var: &serde_json::Value,
        origin: TaskOrigin,
    ) -> Result<(TaskId, ChainIndex)> {
        let chain = &self.chains[id.index()];
        let depth = chain.call_depth + 1;
        if depth > MAX_CALL_DEPTH {
            return Err(Error::Lua(format!(
                "call recursion exceeded cap of {MAX_CALL_DEPTH}"
            )));
        }
        // An explicit input forks the chain's args (and `argv` re-derives
        // from them), as a `call` with input does; otherwise the chain
        // inherits the spawner's context whole. The turn counter is the
        // task's own, as a fanout arm's is.
        let child_ctx = match input {
            Some(input) => chain.ctx.with_args(input),
            None => chain.ctx.clone(),
        };
        let child_ctx = child_ctx.with_effective_handles(
            Arc::clone(child_ctx.observer()),
            child_ctx.debug().cloned(),
            Arc::new(AtomicU32::new(0)),
        );
        let client = chain.client.clone();
        let spawner_access = Arc::clone(chain.access()?);
        let observer = Arc::clone(chain.ctx.observer());
        let execution = chain.ctx.execution().to_owned();
        let spawner_section = chain.section_name().to_owned();
        // `chain`'s arena borrow ends here; the resolution borrows the
        // prompt tree, so the target's slice outlives it.
        let target_section = self.resolve_chain_target(id, target)?;
        let worker = &target_section.slice[target_section.index];
        if worker.prologue().is_none() && worker.epilog().is_none() && !worker.items().is_empty() {
            return Err(Error::Lua(format!(
                "section `{}` is a list section, not a worker template",
                worker.name()
            )));
        }
        // The access spawns before the chain exists: a store refusal here
        // is the last fallible step that can leave nothing behind, so it
        // runs ahead of the id allocation and the arena push rather than
        // orphaning a started chain that is neither enqueued nor slotted.
        let access = spawner_access
            .spawn(prompt_origin(
                self.ctx.prompt(),
                worker.name(),
                worker.blocks(),
            ))
            .map_err(Error::Store)?;
        // The task's id is the spawner's next child index, shared with
        // `call` children, so it depends only on the spawner's own
        // dispatch order; the task is its chain, named from the other side.
        let chain_id = self.allocate_child_id(id)?;
        let task = TaskId::from(chain_id.clone());
        let child = self.start_chain(
            chain_id,
            Counters::default(),
            child_ctx,
            target_section.slice,
            target_section.index,
            None,
            var,
            depth,
            None,
        )?;
        let spawned = &mut self.chains[child.index()];
        spawned.access = Some(Arc::new(access));
        spawned.client = client;
        spawned.task = task.clone();
        spawned.owner = Some(id);
        spawned.seed = Some(seed.clone());
        self.tasks.insert(
            task.clone(),
            TaskSlot {
                backing: TaskBacking::Chain(child),
                owner: id,
                origin,
                target: worker.name().to_owned(),
                state: TaskState::Running,
                outcome: None,
            },
        );
        // The start carries the spawn seeds: everything a host needs to
        // start the same chain again under the same id.
        observer.observe(
            &execution,
            &spawner_section,
            Observation::TaskStarted {
                task: task.clone(),
                target: worker.name().to_owned(),
                origin,
                input: input.map(str::to_owned),
                item: seed.item,
                index: seed.index,
                var: var.clone(),
            },
        );
        Ok((task, child))
    }

    /// Applies a task chain's end to its slot: the outcome lands in the
    /// slot, the slot moves to `Done`, and the task's terminal observation
    /// fires under its target section. Delivery to a waiting owner is the
    /// wait arms' job; the slot holds the outcome until then.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the chain has no slot, which only a
    /// scheduler bug produces: every task chain registers its slot before
    /// it is enqueued.
    pub(super) fn complete_task(&mut self, id: ChainIndex, outcome: Result<String>) -> Result<()> {
        let chain = &self.chains[id.index()];
        let task = chain.task.clone();
        let observer = Arc::clone(chain.ctx.observer());
        let execution = chain.ctx.execution().to_owned();
        let Some(slot) = self.tasks.get_mut(&task) else {
            return Err(Error::internal("a task chain's end implies its slot"));
        };
        let event = if outcome.is_ok() {
            Observation::TaskSucceeded { task }
        } else {
            Observation::TaskFailed { task }
        };
        slot.state = TaskState::Done;
        slot.outcome = Some(outcome);
        observer.observe(&execution, &slot.target, event);
        Ok(())
    }
}
