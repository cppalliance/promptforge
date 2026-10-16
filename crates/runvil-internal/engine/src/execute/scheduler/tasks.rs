//! The task arena and the `spawn` arm. A task is
//! a chain the scheduler
//! runs beside its spawner instead of in place of it: `tasks.spawn` (and
//! the `fanout` shim, once per arm) starts the chain over the target's
//! slice - under `call`'s target resolution and depth cap, refusing a list
//! section as the target - registers a slot for it keyed by the chain's
//! own hierarchical id, and resumes the spawner at once with the id. The
//! spawner runs first; the child waits in the admission queue, holding no
//! Lua VM, until the run's concurrency limits admit it.
//!
//! The slot outlives the chain. When the chain ends, its outcome lands in
//! the slot and the slot moves to `Done`; the owner later takes the result
//! through a wait, which moves it to `Delivered`. Cancellation and
//! abandonment (the owner ending first) are the two other terminal states,
//! kept apart because the log and the model notice must tell "stopped on
//! purpose" from "lost its owner". A terminal slot is never removed: the
//! arena is append-only as the chain arena is, so a late `status` can
//! still report how a task ended.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use runvil_types::ids::{TaskId, TaskOrigin};
use runvil_vfs::{ExecId, detail};

use crate::error::DepthVia;
use crate::execute::protocol::{Answer, Blocked, SpawnVia};
use crate::execute::run::EffectId;
use crate::execute::section_context::TaskSeed;
use crate::{Error, Result};

use super::chain::ChainStart;
use super::chain_record::{Admission, ChainKind};
use super::{ChainIndex, Counters, Scheduler, SpawnRecord, prompt_origin};

/// Where a task's work runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TaskBacking {
    /// A chain in the arena: every author- or model-started task.
    Chain(ChainIndex),
    /// An in-flight leaf effect: the internal timer behind a wait's
    /// timeout, never author-visible.
    Effect(EffectId),
}

/// One task's lifecycle state, with whether it ended well once it has
/// ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskState {
    /// The backing chain or request is live.
    Running,
    /// The backing chain ended; its outcome waits in the slot. `ok` is
    /// whether the chain returned rather than failed.
    Done { ok: bool },
    /// The owner took the outcome through a wait. `ok` is the outcome's,
    /// kept so `status` still reports it after the wait took it.
    Delivered { ok: bool },
    /// The owner cancelled the task.
    Cancelled,
    /// The owner ended while the task was live, so the Engine ended it.
    Abandoned,
}

impl TaskState {
    /// True while the task's backing chain or request is still running:
    /// the one state a chain end must act on.
    pub(super) fn is_live(self) -> bool {
        matches!(self, TaskState::Running)
    }

    /// Whether the task ended well, once it has ended: a cancelled or
    /// abandoned task did not.
    pub(super) fn ok(self) -> Option<bool> {
        match self {
            TaskState::Running => None,
            TaskState::Done { ok } | TaskState::Delivered { ok } => Some(ok),
            TaskState::Cancelled | TaskState::Abandoned => Some(false),
        }
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
    /// into the spawner so an author `pcall` can catch it. `via` names the
    /// call that started the task: a `fanout` arm's depth-cap refusal is
    /// named after `tasks.fanout`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the spawn keeps the request's target, input, seeds, var snapshot, origin, and starting call explicit"
    )]
    pub(super) fn dispatch_spawn(
        &mut self,
        id: ChainIndex,
        target: &str,
        input: Option<&str>,
        seed: TaskSeed,
        var: serde_json::Value,
        origin: TaskOrigin,
        via: SpawnVia,
    ) {
        match self.prepare_spawn(id, target, input, seed, var, origin, via) {
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
    /// `tasks.fanout` for an arm and `call` otherwise), `call`'s target
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
        reason = "the spawn keeps the request's target, input, seeds, var snapshot, origin, and starting call explicit"
    )]
    pub(super) fn prepare_spawn(
        &mut self,
        id: ChainIndex,
        target: &str,
        input: Option<&str>,
        seed: TaskSeed,
        var: serde_json::Value,
        origin: TaskOrigin,
        via: SpawnVia,
    ) -> Result<(TaskId, ChainIndex)> {
        let chain = &self.chains[id.index()];
        let depth = chain.call_depth.deeper(match via {
            SpawnVia::Fanout => DepthVia::Fanout,
            SpawnVia::Spawn => DepthVia::Call,
        })?;
        // The spawning section, captured now: the start event reports it
        // at admission, which may come after the spawner's walk moved to
        // another section.
        let spawner_section = chain.section_name().to_owned();
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
        let worker = &target_section.slice.resolve(&prompt)[target_section.index.get()];
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
        let access = detail::access_spawn(
            &spawner_access,
            prompt_origin(&prompt, worker.name(), worker.blocks()),
        )
        .map_err(Error::store)?;
        // The task's happens-before identity, recorded before the access
        // moves into the chain: every delivery joins it, and the chain
        // end joins it last.
        let exec = detail::access_id(&access);
        // The task's id is the spawner's next child index, shared with
        // `call` children, so it depends only on the spawner's own
        // dispatch order; the task is its chain, named from the other side.
        let chain_id = self.allocate_child_id(id)?;
        let task = TaskId::from(chain_id.clone());
        // The task's context reports under its own task id with its own
        // turn counter, so its events report its provenance and its turns
        // count against its own cap.
        let child_ctx = child_ctx.with_task(task.clone(), Arc::new(AtomicU32::new(0)));
        // The task waits for admission from here, and the spawn record
        // holds what the start event reports once the task first runs.
        let child = self.start_chain(ChainStart {
            lineage: chain_id,
            counters: Counters::default(),
            ctx: child_ctx,
            slice: target_section.slice,
            index: target_section.index,
            kind: ChainKind::Task {
                owner: id,
                admission: Admission::Queued(SpawnRecord {
                    input: input.map(str::to_owned),
                    section: spawner_section,
                }),
            },
            var,
            call_depth: depth,
            concurrency: child_concurrency,
        })?;
        let spawned = &mut self.chains[child.index()];
        spawned.access = Some(Arc::new(access));
        spawned.seed = Some(seed);
        // `blocked` reads `queued` in `tasks.status` while the slot's
        // state is still `Running`.
        spawned.blocked = Some(Blocked::Queued);
        self.tasks.insert(
            task.clone(),
            TaskSlot {
                backing: TaskBacking::Chain(child),
                owner: id,
                origin,
                target: worker.name().to_owned(),
                state: TaskState::Running,
                outcome: None,
                exec: Some(exec),
            },
        );
        Ok((task, child))
    }
}
