//! The task arena and the `spawn` arm. A task is a chain the scheduler
//! runs beside its spawner instead of in place of it: `tasks.spawn` (and
//! the `fanout` shim, once per arm) starts the chain over the target's
//! slice - under `call`'s target resolution and depth cap, refusing a list
//! section as the target - registers a slot for it keyed by the chain's
//! own hierarchical id, and resumes the spawner at once with the id. The
//! spawner runs first; the child runs when the spawner suspends or ends,
//! exactly as any ready chain does.
//!
//! The slot outlives the chain. When the chain ends, its outcome lands in
//! the slot and the slot moves to `Done`; the owner later takes the result
//! through a wait, which moves it to `Delivered`. Cancellation and
//! abandonment (the owner ending first) are the two other terminal states,
//! kept apart because the log and the model notice must tell "stopped on
//! purpose" from "lost its owner". A terminal slot is never removed: the
//! arena is append-only as the chain arena is, so a late `status` can
//! still report how a task ended.
//!
//! The chain-end rules: a task ends with its owner. When a chain ends,
//! every live task it owns is abandoned - its slot moves to `Abandoned`,
//! its terminal observation names how the owner ended, and its backing
//! chain aborts with everything it owns in turn. An author-origin task
//! left live is the author's bug, so the owner's own outcome becomes
//! `tasks_live` naming the leaked ids; a model-origin task is abandoned
//! quietly (the model learns through a notice). Tasks survive a section's
//! fall-through and a `jump` - those move the walk within one chain - and
//! end with the chain itself: the root walk, a `call` child, or another
//! task (a fanout arm among them). The H1 pass and the walk after it are
//! one chain, so the hand-off reassigns the pass's tasks to the walk.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use promptforge_api_types::ids::{AbandonReason, TaskId, TaskOrigin};

use crate::execute::protocol::Answer;
use crate::execute::section_context::TaskSeed;
use crate::execute::support::MAX_CALL_DEPTH;
use crate::{Error, Result};
use promptforge_api_types::event::Event;

use super::notices::TaskEnd;
use super::{ChainIndex, Counters, Scheduler, prompt_origin};
use crate::execute::run::EffectId;

/// Where a task's work runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TaskBacking {
    /// A chain in the arena: every author- or model-started task.
    Chain(ChainIndex),
    /// An in-flight leaf effect: the internal timer behind a wait's
    /// timeout, never author-visible.
    Effect(EffectId),
}

/// One task's lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskState {
    /// The backing chain or request is live.
    Running,
    /// The backing chain ended; its outcome waits in the slot.
    Done,
    /// The owner took the outcome through a wait.
    Delivered,
    /// The owner cancelled the task.
    Cancelled,
    /// The owner ended while the task was live, so the engine ended it.
    Abandoned,
}

impl TaskState {
    /// True while the task's backing chain or request is still running:
    /// the one state a chain end must act on.
    pub(super) fn is_live(self) -> bool {
        matches!(self, TaskState::Running)
    }
}

/// One task's slot in the arena.
#[derive(Debug)]
pub(super) struct TaskSlot {
    /// Where the task's work runs.
    pub(super) backing: TaskBacking,
    /// The chain that started the task: the only chain allowed to wait on,
    /// inspect, or cancel it, and the chain whose end ends the task.
    pub(super) owner: ChainIndex,
    /// The principal that started the task.
    pub(super) origin: TaskOrigin,
    /// The name of the section the task's chain started at: the section
    /// its terminal observations report under.
    pub(super) target: String,
    /// The task's lifecycle state.
    pub(super) state: TaskState,
    /// Whether the task ended well, once it has ended: `Some(true)` for a
    /// chain that returned, `Some(false)` for one that failed, was
    /// cancelled, or was abandoned. Kept beside `outcome` so `status`
    /// still reports it after a wait took the outcome.
    pub(super) ok: Option<bool>,
    /// The chain's final text or failure, held from the chain's end until
    /// the owner takes it.
    pub(super) outcome: Option<Result<String>>,
}

impl TaskSlot {
    /// True for a slot the author never sees: the effect-backed timer
    /// behind a timed wait. Internal slots are omitted from `pending` and
    /// a status table's `tasks`, report no task observations, and never
    /// count as leaked.
    pub(super) fn is_internal(&self) -> bool {
        matches!(self.backing, TaskBacking::Effect(_))
    }
}

impl Scheduler {
    /// Dispatches a `spawn` request: constructs the task's chain, registers
    /// its slot, and resumes the spawner with the task's id; the child is
    /// enqueued behind the spawner, so `spawn` returns before the child
    /// runs. Every dispatch failure - the depth cap, target resolution, the
    /// worker check, chain construction - is the call's answer, resumed
    /// into the spawner so an author `pcall` can catch it. `fanout` marks
    /// a `fanout` arm, whose depth-cap refusal is named after `fanout`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the spawn keeps the request's target, input, seeds, var snapshot, origin, and fanout mark explicit"
    )]
    pub(super) fn dispatch_spawn(
        &mut self,
        id: ChainIndex,
        target: &str,
        input: Option<&str>,
        seed: TaskSeed,
        var: &serde_json::Value,
        origin: TaskOrigin,
        fanout: bool,
    ) {
        match self.prepare_spawn(id, target, input, seed, var, origin, fanout) {
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

    /// The fallible half of spawn dispatch, shared with the model's `task`
    /// built-in: `call`'s depth cap against the spawner's call-depth field
    /// (the refusal named after the author-facing call that tripped it,
    /// `fanout` for an arm and `call` otherwise, so the text is the one
    /// each path always had), `call`'s target resolution over the
    /// spawner's visible set, the worker-template check (a list section is
    /// not a target), then the task chain one level deeper under the
    /// spawn's `args` and `var` snapshot, with its own access capability
    /// (a concurrent thread of execution under the claims model, spawned
    /// from the spawner's so the spawn is the happens-before edge) and a
    /// fresh turn counter. The caller enqueues the returned child behind
    /// the spawner.
    #[expect(
        clippy::too_many_arguments,
        reason = "the spawn keeps the request's target, input, seeds, var snapshot, origin, and fanout mark explicit"
    )]
    pub(super) fn prepare_spawn(
        &mut self,
        id: ChainIndex,
        target: &str,
        input: Option<&str>,
        seed: TaskSeed,
        var: &serde_json::Value,
        origin: TaskOrigin,
        fanout: bool,
    ) -> Result<(TaskId, ChainIndex)> {
        let chain = &self.chains[id.index()];
        let depth = chain.call_depth + 1;
        if depth > MAX_CALL_DEPTH {
            let tripped = if fanout { "fanout" } else { "call" };
            return Err(Error::Lua(format!(
                "{tripped} recursion exceeded cap of {MAX_CALL_DEPTH}"
            )));
        }
        // An explicit input forks the chain's args (and `argv` re-derives
        // from them), as a `call` with input does; otherwise the chain
        // inherits the spawner's context whole. The turn counter is the
        // task's own, so its turns count against its own cap.
        let child_ctx = match input {
            Some(input) => chain.ctx.with_args(input),
            None => chain.ctx.clone(),
        };
        let spawner_access = Arc::clone(chain.access()?);
        let spawner_emitter = Arc::clone(chain.ctx.emitter());
        let spawner_section = chain.section_name().to_owned();
        // `chain`'s arena borrow ends here; the resolution names the
        // target's slice by path, resolved against the shared tree.
        let prompt = self.prompt();
        let target_section = self.resolve_chain_target(id, target)?;
        let worker = &target_section.slice.resolve(&prompt)[target_section.index];
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
            .spawn(prompt_origin(&prompt, worker.name(), worker.blocks()))
            .map_err(Error::Store)?;
        // The task's id is the spawner's next child index, shared with
        // `call` children, so it depends only on the spawner's own
        // dispatch order; the task is its chain, named from the other side.
        let chain_id = self.allocate_child_id(id)?;
        let task = TaskId::from(chain_id.clone());
        // The task's context reports under its own task id with its own
        // turn counter, so its events carry its provenance and its turns
        // count against its own cap.
        let child_ctx = child_ctx.with_task(task.clone(), Arc::new(AtomicU32::new(0)));
        let child = self.start_chain(
            chain_id,
            Counters::default(),
            child_ctx,
            target_section.slice,
            target_section.index,
            None,
            var,
            depth,
        )?;
        let spawned = &mut self.chains[child.index()];
        spawned.access = Some(Arc::new(access));
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
                ok: None,
                outcome: None,
            },
        );
        // The start carries the spawn seeds: everything a host needs to
        // start the same chain again under the same id. The spawn is the
        // spawner's act, so it rides the spawner's task sequence.
        spawner_emitter.emit(&spawner_section, |execution, section, provenance| {
            Event::TaskStarted {
                execution,
                section,
                provenance,
                task: task.clone(),
                target: worker.name().to_owned(),
                origin,
                input: input.map(str::to_owned),
                item: seed.item,
                index: seed.index,
                var: var.clone(),
            }
        });
        Ok((task, child))
    }

    /// Applies a task chain's end to its slot: the outcome lands in the
    /// slot, the slot moves to `Done`, the task's terminal observation
    /// fires under its target section, a model task's notice is queued on
    /// its owner, and an owner parked on a set containing the task is
    /// woken with it delivered (the notice is queued first, so a model
    /// parked in `await_tasks` reads it in the wake's answer). Otherwise
    /// the slot holds the outcome until a wait takes it.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the chain has no slot, which only a
    /// scheduler bug produces: every task chain registers its slot before
    /// it is enqueued.
    pub(super) fn complete_task(&mut self, id: ChainIndex, outcome: Result<String>) -> Result<()> {
        let chain = &self.chains[id.index()];
        let task = chain.task.clone();
        // The terminal is the task's own last word, stamped with its task.
        let emitter = Arc::clone(chain.ctx.emitter());
        let Some(slot) = self.tasks.get_mut(&task) else {
            return Err(Error::internal("a task chain's end implies its slot"));
        };
        let succeeded = outcome.is_ok();
        slot.state = TaskState::Done;
        slot.ok = Some(succeeded);
        let owner = slot.owner;
        let origin = slot.origin;
        let target = slot.target.clone();
        emitter.emit(&target, |execution, section, provenance| {
            let task = task.clone();
            if succeeded {
                Event::TaskSucceeded {
                    execution,
                    section,
                    provenance,
                    task,
                }
            } else {
                Event::TaskFailed {
                    execution,
                    section,
                    provenance,
                    task,
                }
            }
        });
        if origin == TaskOrigin::Model {
            let end = match &outcome {
                Ok(text) => TaskEnd::Completed(text),
                Err(error) => TaskEnd::Failed(error),
            };
            self.queue_task_notice(owner, &task, &target, end);
        }
        if let Some(slot) = self.tasks.get_mut(&task) {
            slot.outcome = Some(outcome);
        }
        self.wake_waiter(owner, &task);
        Ok(())
    }

    /// Applies the chain-end rules for tasks to `owner`'s `outcome`: every
    /// live task the chain owns is abandoned (the reason names how the
    /// owner ended: the section ended, the tool loop was exhausted, or the
    /// owner failed some other way), and an `Ok` outcome that leaked
    /// author-origin tasks becomes [`Error::TasksLive`] naming them in
    /// spawn order. A failing chain keeps its own error - the leak is the
    /// lesser fault - but its tasks end all the same.
    pub(super) fn settle_owned_tasks(
        &mut self,
        owner: ChainIndex,
        outcome: Result<String>,
    ) -> Result<String> {
        let reason = match &outcome {
            Ok(_) => AbandonReason::OwnerReturned,
            Err(Error::ToolLoopExhausted) => AbandonReason::ToolLoopExhausted,
            Err(_) => AbandonReason::OwnerFailed,
        };
        let leaked = self.abandon_owned_tasks(owner, reason);
        match outcome {
            Ok(_) if !leaked.is_empty() => Err(Error::TasksLive { tasks: leaked }),
            outcome => outcome,
        }
    }

    /// Ends every live task `owner` owns because `owner` is ending: each
    /// slot moves to `Abandoned`, its backing chain aborts with everything
    /// it owns in turn (or its in-flight request is dropped), and its
    /// terminal observation fires under its target with `reason` - the
    /// observation is the reason's record, since no wait can reach an
    /// abandoned slot once its owner is gone; a model task's abandonment
    /// notice is queued and reported too, though the ending owner never
    /// reads it. An internal timer slot ends the same way but reports
    /// nothing and never counts as leaked: it is the wait's detail, not a
    /// task the author started. Returns the abandoned author-origin ids in
    /// spawn order, for the owner's `tasks_live` outcome; the caller
    /// discards them for an owner that is itself being aborted, whose
    /// outcome no one receives.
    pub(super) fn abandon_owned_tasks(
        &mut self,
        owner: ChainIndex,
        reason: AbandonReason,
    ) -> Vec<TaskId> {
        // The arena is a hash map; ids order as paths and one owner's tasks
        // are its direct children, so sorting recovers spawn order.
        let mut live: Vec<(TaskId, TaskOrigin, TaskBacking, String)> = self
            .tasks
            .iter()
            .filter(|(_, slot)| slot.owner == owner && slot.state.is_live())
            .map(|(task, slot)| (task.clone(), slot.origin, slot.backing, slot.target.clone()))
            .collect();
        live.sort_by(|left, right| left.0.cmp(&right.0));
        let mut leaked = Vec::new();
        for (task, origin, backing, target) in live {
            if let Some(slot) = self.tasks.get_mut(&task) {
                slot.state = TaskState::Abandoned;
                slot.ok = Some(false);
            }
            // The backing ends first, so anything it owned reports before
            // the task's own terminal event, which is the last word on it.
            let backing_chain = match backing {
                TaskBacking::Chain(backing_chain) => backing_chain,
                TaskBacking::Effect(effect) => {
                    self.abort_effect(effect);
                    continue;
                }
            };
            // The terminal is stamped with the abandoned task's own
            // provenance: its backing chain's emitter, taken before the
            // abort clears the chain's state.
            let emitter = Arc::clone(self.chains[backing_chain.index()].ctx.emitter());
            self.abort_subtree(backing_chain);
            emitter.emit(&target, |execution, section, provenance| {
                Event::TaskAbandoned {
                    execution,
                    section,
                    provenance,
                    task: task.clone(),
                    reason,
                }
            });
            match origin {
                TaskOrigin::Author => leaked.push(task),
                TaskOrigin::Model => {
                    self.queue_task_notice(owner, &task, &target, TaskEnd::Abandoned(reason));
                }
            }
        }
        leaked
    }

    /// Moves every task `from` owns to `to`, with the notices not yet
    /// delivered: the H1 hand-off, where the pass and the walk are one
    /// chain (`0`) on either side, so a task the pass spawned is waited
    /// on, inspected, cancelled, or leaked by the walk exactly as if the
    /// walk had spawned it.
    pub(super) fn reassign_tasks(&mut self, from: ChainIndex, to: ChainIndex) {
        for slot in self.tasks.values_mut() {
            if slot.owner == from {
                slot.owner = to;
            }
        }
        // An effect-backed slot's effect is keyed under its owner in the
        // pending table; the pass has no parked effect of its own at the
        // hand-off, so every entry under it is such a slot's.
        for pending in self.pending.values_mut() {
            if pending.chain == from {
                pending.chain = to;
            }
        }
        for chain in &mut self.chains {
            if chain.owner == Some(from) {
                chain.owner = Some(to);
            }
        }
        let notices = std::mem::take(&mut self.chains[from.index()].task_notices);
        self.chains[to.index()].task_notices = notices;
    }
}
