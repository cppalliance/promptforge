//! The task arena, the `spawn` arm, and the admission limits. A task is
//! a chain the scheduler
//! runs beside its spawner instead of in place of it: `tasks.spawn` (and
//! the `fanout` shim, once per arm) starts the chain over the target's
//! slice - under `call`'s target resolution and depth cap, refusing a list
//! section as the target - registers a slot for it keyed by the chain's
//! own hierarchical id, and resumes the spawner at once with the id. The
//! spawner runs first; the child waits in the admission queue, holding no
//! Lua VM, until the run's concurrency limits admit it.
//!
//! Admission bounds the run: every task chain takes a slot at its owner
//! and at every enclosing ancestor, held until the task ends - except
//! while it, or a `call` chain it is blocked on, is parked on a task
//! wait, which gives the slot back so its descendants can run - and a
//! chain admits at most its effective limit at once. The root's limit is the run's
//! [`RunLimits::max_concurrency`](crate::execute::RunLimits::max_concurrency)
//! ceiling; a spawned task and a call chain start with their parent's,
//! and `tasks.concurrency` lowers it, clamped to the parent's. Queue
//! order is spawn order, with resumptions first. The task's start event
//! fires at admission, when the task first runs.
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

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use promptforge_types::ids::{AbandonReason, TaskId, TaskOrigin};
use promptforge_vfs::detail::{access_id, access_spawn};
use promptforge_vfs::{Access, ExecId};

use crate::execute::protocol::Answer;
use crate::execute::section_context::TaskSeed;
use crate::execute::support::MAX_CALL_DEPTH;
use crate::{Error, Result};
use promptforge_types::event::Event;

use super::notices::TaskEnd;
use super::{ChainIndex, Counters, Scheduler, SpawnRecord, prompt_origin};
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
    /// The task's happens-before identity: the [`ExecId`] its access was
    /// spawned under, recorded at spawn so a delivery can join the task
    /// after its chain has dropped the access. `None` for the internal
    /// timer, which has no identity to join.
    pub(super) exec: Option<ExecId>,
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
        var: serde_json::Value,
        origin: TaskOrigin,
        fanout: bool,
    ) {
        match self.prepare_spawn(id, target, input, seed, var, origin, fanout) {
            Ok((task, child)) => {
                self.answer_inline(id, Answer::Spawn(Ok(task)));
                // The child waits for admission, queued behind the
                // spawner; the drain admits it when a slot frees up at
                // its owner and every ancestor.
                self.spawned.push_back(child);
            }
            Err(error) => {
                self.answer_inline(id, Answer::Spawn(Err(error)));
            }
        }
    }

    /// The fallible half of spawn dispatch, shared with the model's `task`
    /// built-in: `call`'s depth cap against the spawner's call-depth field
    /// (the refusal named after the author-facing call that tripped it,
    /// `fanout` for an arm and `call` otherwise), `call`'s target
    /// resolution over the spawner's visible set, the worker-template
    /// check (a list section is not a target), then the task chain one
    /// level deeper under the spawn's `args` and `var` snapshot, with its
    /// own access capability (a concurrent thread of execution, forked
    /// from the spawner's so the spawn is the happens-before edge; every
    /// delivery of the task joins it back) and a fresh turn counter. The
    /// child leaves this call queued for admission - holding no Lua VM
    /// yet - with its `var` snapshot moved into the chain and its start
    /// record stashed; the drain admits it when a slot frees up.
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
        var: serde_json::Value,
        origin: TaskOrigin,
        fanout: bool,
    ) -> Result<(TaskId, ChainIndex)> {
        let chain = &self.chains[id.index()];
        let depth = chain.call_depth + 1;
        // The spawning section, captured now: the start event reports it
        // at admission, which may come after the spawner's walk moved to
        // another section.
        let spawner_section = chain.section_name().to_owned();
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
        // A spawned chain's effective limit starts with its spawner's:
        // its tasks run within the spawner's share and every ancestor's.
        let child_concurrency = chain.concurrency;
        let spawner_access = Arc::clone(chain.access()?);
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
        let access = access_spawn(
            &spawner_access,
            prompt_origin(&prompt, worker.name(), worker.blocks()),
        )
        .map_err(Error::store)?;
        // The task's happens-before identity, recorded before the access
        // moves into the chain: every delivery joins it, and the chain
        // end joins it last.
        let exec = access_id(&access);
        // The task's id is the spawner's next child index, shared with
        // `call` children, so it depends only on the spawner's own
        // dispatch order; the task is its chain, named from the other side.
        let chain_id = self.allocate_child_id(id)?;
        let task = TaskId::from(chain_id.clone());
        // The task's context reports under its own task id with its own
        // turn counter, so its events report its provenance and its turns
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
            child_concurrency,
        )?;
        let spawned = &mut self.chains[child.index()];
        spawned.access = Some(Arc::new(access));
        spawned.task = task.clone();
        spawned.owner = Some(id);
        spawned.seed = Some(seed);
        // The task waits for admission from here: `blocked` reads
        // `queued` in `tasks.status` (the slot's state is still
        // `Running`), and the spawn record holds what the start event
        // reports once the task first runs.
        spawned.blocked = Some("queued");
        spawned.pending_spawn = Some(SpawnRecord {
            input: input.map(str::to_owned),
            section: spawner_section,
        });
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
                exec: Some(exec),
            },
        );
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

    /// Joins `task` into `owner`: merges the task's final clock into the
    /// owner's access, so everything the task did happens before the
    /// owner's next step. A timer (no identity) and an owner already
    /// ended (no access) need no join.
    pub(super) fn join_task(&self, owner: ChainIndex, task: &TaskId) {
        let Some(exec) = self.tasks.get(task).and_then(|slot| slot.exec) else {
            return;
        };
        let Ok(access) = self.chains[owner.index()].access() else {
            return;
        };
        promptforge_vfs::detail::access_join(access, exec);
    }

    /// Joins every task `owner` owns into `access`: the chain-end join,
    /// so the chain's own final clock transitively covers everything its
    /// tasks did even when no wait delivered them.
    pub(super) fn join_owned_tasks(&self, owner: ChainIndex, access: &Access) {
        let execs: Vec<ExecId> = self
            .tasks
            .values()
            .filter(|slot| slot.owner == owner)
            .filter_map(|slot| slot.exec)
            .collect();
        for exec in execs {
            promptforge_vfs::detail::access_join(access, exec);
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
            // abort clears the chain's state. A task that was never
            // admitted (still queued for a slot) has no start event, so
            // it reports no terminal either.
            let admitted = self.chains[backing_chain.index()].admitted;
            let emitter = Arc::clone(self.chains[backing_chain.index()].ctx.emitter());
            self.abort_subtree(backing_chain);
            if admitted {
                emitter.emit(&target, |execution, section, provenance| {
                    Event::TaskAbandoned {
                        execution,
                        section,
                        provenance,
                        task: task.clone(),
                        reason,
                    }
                });
            }
            match origin {
                TaskOrigin::Model => {
                    self.queue_task_notice(owner, &task, &target, TaskEnd::Abandoned(reason));
                }
                // `Author`, or an origin `promptforge-types` adds behind
                // its `#[non_exhaustive]` `TaskOrigin`: treated as the
                // author's. `#[non_exhaustive]` denies this crate an
                // exhaustive match, so a new variant lands here silently.
                // Only one test pins which origins leak:
                // `an_ending_owner_leaks_its_author_task_and_never_its_model_task`.
                _ => leaked.push(task),
            }
        }
        // The abandoned tasks' slots are free: the drain admits the
        // queued chains that now fit at its next edge.
        leaked
    }

    /// Ends every live task in the arena because the run itself is ending:
    /// the whole-run counterpart of the per-owner chain-end rule, so a
    /// task stranded by a host cancel or a fatal answer still receives its
    /// one terminal before the run's end boundary. Each live slot's owner
    /// is passed to [`abandon_owned_tasks`](Self::abandon_owned_tasks)
    /// with `reason`, in ascending arena order. No slot reports twice:
    /// abandoning a task aborts its backing chain, and `abort_subtree`
    /// abandons that chain's own tasks (as `OwnerAborted`) on the way, so
    /// a nested slot is already terminal when its owner's turn comes and
    /// `is_live()` skips it. The leaked-author list is discarded: no one
    /// receives an outcome for a run that is ending.
    pub(super) fn settle_all_tasks(&mut self, reason: AbandonReason) {
        let mut owners: Vec<ChainIndex> = self
            .tasks
            .values()
            .filter(|slot| slot.state.is_live())
            .map(|slot| slot.owner)
            .collect();
        owners.sort_unstable_by_key(|owner| owner.index());
        owners.dedup();
        for owner in owners {
            self.abandon_owned_tasks(owner, reason);
        }
    }

    /// Moves every task `from` owns to `to`, with the notices not yet
    /// delivered: the H1 hand-off, where the pass and the walk are one
    /// chain (`0`) on either side, so a task the pass spawned is waited
    /// on, inspected, cancelled, or leaked by the walk exactly as if the
    /// walk had spawned it. The pass's admission accounting moves with
    /// the tasks: the walk inherits the slots the pass's tasks hold and
    /// the limit that gates them.
    pub(super) fn reassign_tasks(&mut self, from: ChainIndex, to: ChainIndex) {
        for slot in self.tasks.values_mut() {
            if slot.owner == from {
                slot.owner = to;
            }
        }
        self.chains[to.index()].slots_used += self.chains[from.index()].slots_used;
        self.chains[from.index()].slots_used = 0;
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

    /// The chain enclosing `id` in the task tree: its spawner for a task
    /// chain, its call parent for a call chain, `None` for the root.
    pub(super) fn enclosing(&self, id: ChainIndex) -> Option<ChainIndex> {
        let chain = &self.chains[id.index()];
        chain.owner.or(chain.parent)
    }

    /// Whether the queued task chain `id` can be admitted now: a free
    /// slot at its owner and at every enclosing ancestor, so the task
    /// counts against each chain's limit on the way up.
    fn can_admit(&self, id: ChainIndex) -> bool {
        let mut at = self.chains[id.index()].owner;
        while let Some(ancestor) = at {
            let chain = &self.chains[ancestor.index()];
            if chain.slots_used >= chain.concurrency {
                return false;
            }
            at = chain.owner.or(chain.parent);
        }
        true
    }

    /// Takes one admission slot at the task's owner and every enclosing
    /// ancestor, as [`can_admit`](Self::can_admit) checked.
    fn take_slots(&mut self, id: ChainIndex) {
        let mut at = self.chains[id.index()].owner;
        while let Some(ancestor) = at {
            let chain = &mut self.chains[ancestor.index()];
            chain.slots_used += 1;
            at = chain.owner.or(chain.parent);
        }
    }

    /// Gives back the slots a running task held at its owner and every
    /// enclosing ancestor.
    fn release_slots(&mut self, id: ChainIndex) {
        let mut at = self.chains[id.index()].owner;
        while let Some(ancestor) = at {
            let chain = &mut self.chains[ancestor.index()];
            debug_assert!(
                chain.slots_used > 0,
                "a chain holding a slot cannot release below zero"
            );
            chain.slots_used = chain.slots_used.saturating_sub(1);
            at = chain.owner.or(chain.parent);
        }
    }

    /// Releases `id`'s held admission slots when it holds any: the shared
    /// half of the chain-end paths and a task wait's park.
    pub(super) fn release_chain_slots(&mut self, id: ChainIndex) {
        let holding = self.chains[id.index()].holding;
        if holding {
            self.release_slots(id);
            self.chains[id.index()].holding = false;
        }
    }

    /// Admits every queued task that can take a slot now: resumptions
    /// first (a task resumed from a join takes its slots back ahead of
    /// tasks that have not started, so a resume cannot starve behind a
    /// long queue), then fresh spawns in spawn order.
    pub(super) fn admit(&mut self) {
        let mut remaining = VecDeque::new();
        for id in std::mem::take(&mut self.resuming) {
            if self.can_admit(self.slot_holder(id)) {
                self.resume_chain(id);
            } else {
                remaining.push_back(id);
            }
        }
        self.resuming = remaining;
        let mut remaining = VecDeque::new();
        for id in std::mem::take(&mut self.spawned) {
            if self.can_admit(id) {
                self.admit_chain(id);
            } else {
                remaining.push_back(id);
            }
        }
        self.spawned = remaining;
    }

    /// The chain whose slots `id`'s resumption takes back: the holding
    /// ancestor a parked call chain released, or the resumed task itself.
    fn slot_holder(&self, id: ChainIndex) -> ChainIndex {
        self.chains[id.index()].released_holder.unwrap_or(id)
    }

    /// Re-admits a resumed chain: takes its slot holder's slots back and
    /// enqueues it. Its start event already fired at its first admission -
    /// this path only moves it from its wait back to running.
    fn resume_chain(&mut self, id: ChainIndex) {
        let holder = self.slot_holder(id);
        self.take_slots(holder);
        self.chains[holder.index()].holding = true;
        let chain = &mut self.chains[id.index()];
        chain.released_holder = None;
        chain.blocked = None;
        self.ready.push_back(id);
    }

    /// Admits one queued task chain: takes a slot at its owner and every
    /// enclosing ancestor, installs its spawn record, fires the start
    /// event on the spawner's sequence under the spawning section (the
    /// spawn-time capture, so a task admitted after its spawner moved on
    /// still reports where it was spawned), and enqueues it. The start
    /// event marks admission - the task first runs now - so a task still
    /// waiting for a slot reports no start event, and its Lua VM is only
    /// created at its first section entry, after admission.
    fn admit_chain(&mut self, id: ChainIndex) {
        self.take_slots(id);
        let (spawner, task, input, item, index, var, section) = {
            let chain = &mut self.chains[id.index()];
            chain.holding = true;
            chain.admitted = true;
            chain.blocked = None;
            let spawner = chain
                .owner
                .unwrap_or_else(|| unreachable!("a queued task chain has a spawner"));
            let record = chain
                .pending_spawn
                .take()
                .unwrap_or_else(|| unreachable!("a queued task chain holds its spawn record"));
            (
                spawner,
                chain.task.clone(),
                record.input,
                chain.seed.as_ref().and_then(|seed| seed.item.clone()),
                chain.seed.as_ref().and_then(|seed| seed.index),
                chain.var.clone(),
                record.section,
            )
        };
        let (target, origin, emitter) = {
            let slot = self
                .tasks
                .get(&task)
                .unwrap_or_else(|| unreachable!("a task chain has a slot"));
            let spawner_chain = &self.chains[spawner.index()];
            (
                slot.target.clone(),
                slot.origin,
                Arc::clone(spawner_chain.ctx.emitter()),
            )
        };
        // The start event fires at admission, on the spawner's sequence:
        // the task first runs now, and the payload's seeds are enough to
        // start the same chain again under the same id.
        emitter.emit(&section, |execution, section, provenance| {
            Event::TaskStarted {
                execution,
                section,
                provenance,
                task,
                target,
                origin,
                input,
                item,
                index,
                var,
            }
        });
        self.ready.push_back(id);
    }

    /// A chain parked on a task wait gives its admission slots back when
    /// the wait holds real tasks - a timer-only wait keeps them, since
    /// nothing it waits for needs a slot - so its descendants can run
    /// under its limit. A call chain holds no slots of its own, so it
    /// gives back those of its nearest holding ancestor through `parent`
    /// (the task it runs inside, blocked on the call) and records which
    /// chain gave them. A call chain in the main walk finds no holding
    /// ancestor, and the main walk holds no slots to give.
    pub(super) fn park_wait(&mut self, id: ChainIndex) {
        let chain = &self.chains[id.index()];
        let waits_on_tasks = chain
            .waiting_on
            .iter()
            .any(|task| self.tasks.get(task).is_some_and(|slot| !slot.is_internal()));
        if !waits_on_tasks {
            return;
        }
        if chain.holding {
            self.release_chain_slots(id);
            return;
        }
        let mut at = chain.parent;
        while let Some(ancestor) = at {
            let chain = &self.chains[ancestor.index()];
            if chain.holding {
                self.release_chain_slots(ancestor);
                self.chains[id.index()].released_holder = Some(ancestor);
                return;
            }
            at = chain.parent;
        }
    }

    /// Resumes a chain parked on a task wait with its delivered answer:
    /// a task chain that gave its slots back, and a call chain that gave
    /// back its ancestor's, take them again - admitted ahead of fresh
    /// starts at the drain's edge - while the main walk, and a chain that
    /// kept its slots (a timer-only wait), resume inline.
    pub(super) fn wake_from_wait(&mut self, id: ChainIndex, answer: Answer<Error>) {
        let chain = &self.chains[id.index()];
        let requeue = chain.released_holder.is_some() || (!chain.holding && chain.owner.is_some());
        let chain = &mut self.chains[id.index()];
        chain.incoming = Some(answer);
        if requeue {
            // Waiting for a slot again reads `queued`, exactly as a
            // fresh spawn's wait does.
            chain.blocked = Some("queued");
            self.resuming.push_back(id);
        } else {
            self.ready.push_back(id);
        }
    }
}
