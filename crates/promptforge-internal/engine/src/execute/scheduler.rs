//! The chain scheduler: the coroutine protocol's state machine.
//!
//! One [`Scheduler`] per run, owned by the [`Run`](super::run::Run) that
//! the caller steps: no `Arc`, no `Mutex`, no sharing. One caller at a time
//! runs every chain step, and the Lua shims only yield (they never call
//! into Rust for suspending operations), so the scheduler state is
//! unreachable from Lua. Nothing here awaits, spawns, or sleeps: a leaf
//! request becomes an [`Effect`] the step hands out, and the caller's
//! [`EffectAnswer`] comes back through `resume`. The scheduler is `Send`
//! and moves between threads between calls.
//!
//! The loop is `resume -> match request -> dispatch -> resume with answer`.
//! A chain whose coroutine yields a leaf request (`infer`) is parked in the
//! pending table while its [`Effect`] is out with the caller: the arm builds
//! the effect as a value, `issue` stamps it with the chain's task
//! provenance and queues it for the step's return, and `apply_answer`
//! turns the caller's answer into the chain's protocol answer on the
//! caller's thread, emitting the round's events there. A chain that
//! yields a structural request (`call`) blocks while its child chain runs,
//! and the child's finish delivers its final text as the parent's answer.
//! When no chain is ready the step returns and the caller performs the effects.
//!
//! [`RunState`] stays the ambient shared read-mostly context, cloned into
//! chains and callbacks; the scheduler owns the run's copy and is the
//! exclusively owned mutable counterpart. The two are deliberately not
//! merged: `RunState` is cloned into callbacks, while the scheduler must
//! stay unreachable from the callback layer. The prompt tree is shared
//! through the context's `Arc`, so a chain names its position in it by
//! path ([`SlicePath`]) rather than by borrow, and the scheduler owns
//! itself outright.
//!
//! This file holds the scheduler core: the chain arena, the
//! ready queue, the admission queues (spawned tasks wait there until
//! the run's concurrency limits admit them), the pending table, the task
//! arena, and
//! the one `issue` path every leaf arm hands its effect through. The
//! submodules hold the rest: `pending` the pending table's entry (the
//! `Continuation` an answer is applied by), `drive` the run-level step
//! (the ready-queue drain, the terminal rules, and the teardown), `apply`
//! the answer application, `chain` the chain lifecycle (arena insertion
//! and the two chain-end paths), `chain_record` the chain record the
//! arena holds, `step` one chain's step to its next
//! suspension point, `walk` the section walk rules, `h1` the live H1 pass
//! and its hand-off to the walk, `dispatch` the request arms, `chat` the
//! one-round `chat` arm and its answer application, `tool_call` the
//! script and model-issued `tool_call` arm (the two arms the
//! section-visible `models.loop` shim drives), `builtins` the model's
//! task built-ins (`task`, `task_cancel`, `task_status`) answered over
//! the arena and advertised once a section runs `tools.allow_tasks`,
//! `await_tasks` the fourth built-in, the model's wait over its live
//! tasks, `notices` the model-task notices
//! (queued at a model task's end, drained into the owner's next round or
//! its `await_tasks` answer), `tasks` the task arena and the `spawn` arm,
//! `admission` the admission limits, `task_end` the chain-end rules for
//! tasks, `waits` the `join_any` wait and the
//! `ready`, `status`, `pending`, `note`, `concurrency`, and `cancel` arms over the arena,
//! `timer` the wait shims' internal timeout as an effect-backed slot, and
//! `test_hooks` (test builds only) the seams the suites inspect the arena
//! through. A fanout is Lua over those arms (the `fanout` shim spawns one
//! task per member, all up front, and waits on the live set).

mod admission;
mod apply;
mod await_tasks;
mod builtins;
mod chain;
mod chain_record;
mod chat;
mod dispatch;
mod drive;
mod h1;
mod notices;
mod pending;
mod step;
mod task_end;
mod tasks;
#[cfg(test)]
pub(crate) mod test_hooks;
mod timer;
mod tool_call;
mod waits;
mod walk;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use promptforge_types::ids::{Provenance, RoundId, TaskId};
use promptforge_vfs::Origin;
use promptforge_vfs::detail::ScopeHandle;

use crate::parser::{Block, Prompt, Section};
use crate::{Error, Result};
use promptforge_types::event::{ReplyOrigin, lifecycle};

use super::context::RunState;
use super::protocol::Answer;
use super::run::{Effect, EffectAnswer, EffectId, Round};
use chain_record::{Chain, ChatAnchor};
use pending::{Continuation, Pending, ToolCallContinuation, VfsContinuation};
use tasks::TaskSlot;
pub(in crate::execute) use tool_call::RESERVED_TOOL_NAMES;

/// Where a sibling slice sits in the prompt tree: the index of each
/// ancestor section from the top level down to the slice's parent. The
/// empty path is the top-level slice. A chain names its walk position by
/// path so it borrows nothing from the tree the run shares through its
/// `Arc<Prompt>`; the path resolves to the slice on demand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SlicePath(Vec<usize>);

impl SlicePath {
    /// The top-level slice.
    fn root() -> Self {
        Self::default()
    }

    /// The slice of `parent`'s children, where `parent` is the section at
    /// `index` of this slice.
    fn child(&self, index: usize) -> Self {
        let mut path = self.0.clone();
        path.push(index);
        Self(path)
    }

    /// Resolves the path against `prompt`. A path the scheduler built is
    /// always in range; an out-of-range index resolves to the empty slice,
    /// which the walk treats as exhausted rather than panicking.
    fn resolve<'p>(&self, prompt: &'p Prompt) -> &'p [Section] {
        let mut slice = promptforge_parser::detail::sections(prompt);
        for &index in &self.0 {
            slice = slice.get(index).map_or(&[], Section::children);
        }
        slice
    }

    /// The name of the slice's section at `index`, or the prompt's title
    /// when `index` is past the slice: a chain's section name before its
    /// first entry.
    fn name_at<'p>(&self, prompt: &'p Prompt, index: usize) -> &'p str {
        self.resolve(prompt)
            .get(index)
            .map_or(prompt.title(), Section::name)
    }
}

/// The most precise prompt-source line known for `blocks`: the first
/// compiled chunk's absolute source line, else the prompt's opening line.
fn first_chunk_line(blocks: &[Block]) -> u32 {
    blocks
        .iter()
        .find_map(|block| match block {
            Block::Lua(program) => Some(program.source_line().get()),
            _ => None,
        })
        .unwrap_or(1)
}

/// The observability origin for a capability the run acquires: `label` is
/// the section or pass the capability serves, and the prompt's title
/// stands in for a file name - a prompt's name is its title, since the
/// source may never have existed on disk.
fn prompt_origin(prompt: &Prompt, label: &str, blocks: &[Block]) -> Origin {
    Origin::at(label, prompt.title(), first_chunk_line(blocks))
}

/// Arena index of a chain: indices, not references, so no chain ever holds
/// a pointer to another. The index is the scheduler's private handle; the
/// chain's identity for authors and callers is its hierarchical
/// [`ChainId`](promptforge_types::ids::ChainId), which never depends on arena order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ChainIndex(u32);

impl ChainIndex {
    /// The arena index as a `usize`.
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// A queued task chain's spawn record, held from spawn until its
/// admission: the input override and the spawning section the start event
/// reports. The `var` snapshot lives in the chain's `var` slot from spawn
/// on, moved once out of the yield - never copied - so a queued chain
/// shares it until admission installs it into the first section.
#[derive(Debug)]
struct SpawnRecord {
    /// The `opts.input` override, reported in the start event.
    input: Option<String>,
    /// The spawner's section at spawn, reported as the start event's
    /// section: a task admitted after its spawner moved on still reports
    /// where it was spawned, not where the spawner is now.
    section: String,
}

/// A chain's local id counters: the indices its next child chain and its
/// next section entry take under its lineage. Set once at chain start:
/// zero for a fresh chain (`Default`), or the values the chain continues
/// from when it takes over an earlier chain's identity (the walk after
/// the H1 pass).
#[derive(Clone, Copy, Debug, Default)]
struct Counters {
    /// The next index a `call` child or a spawned arm takes under the
    /// chain's lineage. The two share the counter.
    next_child: u32,
    /// The next index a section entry takes under the chain's lineage as
    /// its `sys.id`.
    next_entry: u32,
}

/// The run's phase, as the step loop reads it.
enum Phase {
    /// No chain has started: the next step starts the H1 pass or the walk.
    Fresh,
    /// Chains run; the outcome is undecided.
    Running,
    /// The outcome is decided and the run's end boundary reported; `Done`
    /// waits on the outstanding effects.
    Ending(Result<String>),
    /// `Done` was returned.
    Done,
}

/// The coroutine protocol's driver: the chain arena, ready queue, pending
/// table, task arena, and issued-effect queue, owned outright by the run.
pub(crate) struct Scheduler {
    /// The ambient run context, cloned into chains and forked by call
    /// chains.
    ctx: RunState,
    /// The chain arena: append-only, indexed by [`ChainIndex`].
    chains: Vec<Chain>,
    /// Chains eligible to resume (FIFO); the driver drains it before
    /// awaiting anything.
    ready: VecDeque<ChainIndex>,
    /// Task chains waiting for admission, in queue order: resumptions (a
    /// task parked on a task wait that gave its slots back) first, then
    /// fresh spawns in spawn order. A queued chain holds no Lua VM and
    /// no copied `var` snapshot: admission is what starts it.
    resuming: VecDeque<ChainIndex>,
    /// Freshly spawned task chains waiting for admission, in spawn order;
    /// admitted after every resuming chain.
    spawned: VecDeque<ChainIndex>,
    /// One entry per in-flight leaf effect, keyed by the effect's id:
    /// the parked chain and how the answer resumes it.
    pending: HashMap<EffectId, Pending>,
    /// The task arena: one slot per task the run has started, keyed by the
    /// task's id (its backing chain's id). A slot outlives its chain: it
    /// holds the terminal state and the undelivered outcome at least until
    /// the owner takes the result or ends, and in fact for the run - the
    /// arena is append-only like the chain arena, so `status` can report a
    /// terminal state at any later time.
    tasks: HashMap<TaskId, TaskSlot>,
    /// The effects issued since the step began, in issue order, each with
    /// the provenance of the task that built it; the step returns them.
    issued: Vec<(EffectId, Provenance, Effect)>,
    /// The effects whose chain stopped waiting before the caller answered (a
    /// chain end, a task cancel or abandonment, the run's teardown): the
    /// caller still owes each one answer, which is discarded on arrival. An
    /// unknown id that is neither pending nor orphaned means the caller
    /// answered an effect the run never issued, or answered one twice -
    /// which fails loudly rather than passing silently. An id leaves the
    /// set when its answer arrives, so the set stays bounded by the
    /// orphans whose answers have not landed.
    orphaned: HashSet<EffectId>,
    /// The run's phase.
    phase: Phase,
    /// The most chains one run may start: the arena indexes chains by
    /// `u32`, so the count is bounded by the index space. A field rather
    /// than a constant so a test can shrink the bound and drive the
    /// overflow path without allocating the real one.
    max_chains: usize,
    /// The next effect id: a run-wide counter, so every effect the run
    /// issues has a distinct in-flight handle.
    next_effect: u64,
    /// The next round id: a run-wide counter numbering the model rounds
    /// in dispatch order, chat and nested-infer rounds alike.
    next_round: u64,
    /// The run's scope, taken where the run acquires its root identity:
    /// closed when the run reaches `Done` or is dropped before it, so a
    /// store view the caller still holds never outlives the run.
    scope: Option<ScopeHandle>,
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.end_scope();
    }
}

impl Scheduler {
    /// Builds the scheduler for one run over `ctx`'s prompt and reports
    /// the run's start, so the first step's events open with it.
    pub(super) fn new(ctx: RunState) -> Self {
        // The run's boundaries are events like every other report: pushed
        // into the buffer under the root task, so the caller sees them in
        // order with the sections between them.
        ctx.emitter()
            .report(ctx.prompt().title(), lifecycle::RUN_STARTED);
        Self {
            ctx,
            chains: Vec::new(),
            ready: VecDeque::new(),
            resuming: VecDeque::new(),
            spawned: VecDeque::new(),
            pending: HashMap::new(),
            tasks: HashMap::new(),
            issued: Vec::new(),
            orphaned: HashSet::new(),
            phase: Phase::Fresh,
            max_chains: u32::MAX as usize,
            next_effect: 0,
            next_round: 0,
            scope: None,
        }
    }

    /// Ends the run's scope, once: every access still held in it refuses
    /// its next operation, and its claims stop conflicting.
    fn end_scope(&mut self) {
        if let Some(scope) = self.scope.take() {
            promptforge_vfs::detail::end_scope(&scope);
        }
    }

    /// The prompt's shared handle: a chain resolves its walk position
    /// against this clone so the tree borrow never pins the arena.
    fn prompt(&self) -> Arc<Prompt> {
        Arc::clone(self.ctx.prompt_arc())
    }

    /// Whether the run's outcome is decided: the end boundary is reported
    /// and only the orphans' answers stand between the run and `Done`.
    pub(super) fn decided(&self) -> bool {
        matches!(self.phase, Phase::Ending(_) | Phase::Done)
    }

    /// Resumes `id` at once with `answer`: the inline-answer path every
    /// non-waiting task arm takes.
    fn answer_inline(&mut self, id: ChainIndex, answer: Answer<Error>) {
        self.chains[id.index()].incoming = Some(answer);
        self.ready.push_back(id);
    }

    /// Issues one leaf effect for `chain`: allocates its id, stamps it with
    /// the chain's task provenance, queues it for the step's return, and
    /// parks the chain in the pending table with `resume`, the rule its
    /// answer is applied by. The one path every leaf arm takes, so no arm
    /// parks on its own.
    fn issue(&mut self, chain: ChainIndex, effect: Effect, resume: Continuation) -> EffectId {
        let id = EffectId(self.next_effect);
        self.next_effect += 1;
        let provenance = self.chains[chain.index()].ctx.emitter().stamp_effect();
        self.issued.push((id, provenance, effect));
        self.pending.insert(id, Pending { chain, resume });
        id
    }

    /// Numbers the model round about to be issued with `origin`: the run's
    /// next round id, in dispatch order.
    fn number_round(&mut self, origin: ReplyOrigin) -> Round {
        let id = RoundId::new(self.next_round);
        self.next_round += 1;
        Round { id, origin }
    }
}
