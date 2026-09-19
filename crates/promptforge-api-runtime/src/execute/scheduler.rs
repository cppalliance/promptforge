//! The chain-stack scheduler: the coroutine protocol's state machine.
//!
//! One [`Scheduler`] per run, owned by the [`Run`](super::run::Run) that
//! the host steps: no `Arc`, no `Mutex`, no sharing. One caller at a time
//! runs every chain step, and the Lua shims only yield (they never call
//! into Rust for suspending operations), so the scheduler state is
//! unreachable from Lua. Nothing here awaits, spawns, or sleeps: a leaf
//! request becomes an [`Effect`] the step hands out, and the host's
//! [`EffectAnswer`] comes back through `resume`. The scheduler is `Send`
//! and moves between threads between calls.
//!
//! The loop is `resume -> match request -> dispatch -> resume with answer`.
//! A chain whose coroutine yields a leaf request (`infer`) is parked in the
//! pending table while its [`Effect`] is out with the host: the arm builds
//! the effect as a value, `issue` stamps it with the chain's task
//! provenance and queues it for the step's return, and `apply_answer`
//! turns the host's answer into the chain's protocol answer on the
//! caller's thread, emitting the round's events there. A chain that
//! yields a structural request (`call`) blocks while its child chain runs,
//! and the child's finish delivers its final text as the parent's answer.
//! When no chain is ready the step returns and the host performs.
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
//! This file carries the scheduler core: the chain record and arena, the
//! ready queue, the pending table, the call stack, the task arena, and
//! the one `issue` path every leaf arm hands its effect through. The
//! submodules carry the rest: `pending` the pending table's entry (the
//! `Continuation` an answer is applied by), `drive` the run-level step
//! (the ready-queue drain, the terminal rules, and the teardown), `apply`
//! the answer application, `chain` the chain lifecycle (arena insertion
//! and the two chain-end paths), `step` one chain's step to its next
//! suspension point, `walk` the section walk rules, `h1` the live H1 pass
//! and its hand-off to the walk, `dispatch` the request arms, `chat` the
//! one-round `chat` arm and its answer application, `tool_call` the
//! script and model-issued `tool_call` arm (the two arms the
//! section-visible `models.loop` shim drives), `builtins` the model's
//! task built-ins (`task`, `task_cancel`, `task_status`) answered over
//! the arena and advertised once a section runs `tools.allow_tasks`,
//! `await_tasks` the fourth built-in, the model's wait over its live
//! tasks, `task_events` the fifth, the host-answered history read the
//! author's `tasks.events` shares, `notices` the model-task notices
//! (queued at a model task's end, drained into the owner's next round or
//! its `await_tasks` answer), `tasks` the task arena, the `spawn` arm, and
//! the chain-end rules for tasks, `waits` the `when_any` wait and the
//! `ready`, `status`, `pending`, `note`, and `cancel` arms over the arena,
//! `timer` the wait shims' internal timeout as an effect-backed slot, and
//! `test_hooks` (test builds only) the seams the suites inspect the arena
//! through. A fanout is Lua over those arms (the `fanout` shim spawns one
//! task per member and waits on the live set), so the scheduler keeps no
//! fanout state of its own.

mod apply;
mod await_tasks;
mod builtins;
mod chain;
mod chat;
mod dispatch;
mod drive;
mod h1;
mod notices;
mod pending;
mod step;
mod task_events;
mod tasks;
#[cfg(test)]
mod test_hooks;
mod timer;
mod tool_call;
mod waits;
mod walk;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use mlua::Thread;
use promptforge_api_types::ids::{ChainId, Provenance, TaskId};
use shared_vfs::Origin;

use crate::observe::detail;
use crate::parser::{Block, Prompt, Section};
use crate::store::Access;
use crate::{Error, Result};

use super::context::RunState;
use super::protocol::Answer;
use super::run::{Effect, EffectAnswer, EffectId};
use super::scope::DispatchTarget;
use super::section_context::{SectionContext, TaskSeed};
use await_tasks::AwaitTasks;
use pending::{Continuation, Pending, ToolCallContinuation};
use tasks::TaskSlot;
#[cfg(test)]
pub(crate) use tasks::TaskState;

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
        let mut slice = prompt.sections();
        for &index in &self.0 {
            slice = slice.get(index).map_or(&[], Section::children);
        }
        slice
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
/// source may never have lived on disk.
fn prompt_origin(prompt: &Prompt, label: &str, blocks: &[Block]) -> Origin {
    Origin::at(label, prompt.title(), first_chunk_line(blocks))
}

/// Arena index of a chain: indices, not references, so no chain ever holds
/// a pointer to another. The index is the scheduler's private handle; the
/// chain's identity for authors and hosts is its hierarchical
/// [`ChainId`], which never depends on arena order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ChainIndex(u32);

impl ChainIndex {
    /// The arena index as a `usize`.
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// A chain's local id counters: the indices its next child chain and its
/// next section entry take under its lineage. Set once at chain start:
/// zero for a fresh chain (`Default`), or the values the chain continues
/// from when it carries on an earlier chain's identity (the walk after
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

/// One chain: a contained line of section execution, the scheduler's
/// counterpart to the legacy `walk_siblings` invocation.
///
/// The chain owns its per-section frame and adds the chain position (the
/// sibling slice being walked plus the current index), the coroutine handle
/// for the in-flight Lua block, and the walk-scoped slots: the pending
/// Markdown buffer the next Lua fence consumes and the `var` clipboard.
/// One section entry is
/// one frame; the fall-through advance tears the old frame down and the
/// next entry constructs the next.
struct Chain {
    /// The chain's hierarchical id: the parent chain's id extended by the
    /// parent's local child counter (the root chain, the main walk, is
    /// `0`; the H1 pass and the walk that follows it are the same chain).
    /// Every id the chain hands out - its children's, its section
    /// entries' - extends this path, so two runs of one prompt allocate
    /// identical ids however their chains interleave.
    lineage: ChainId,
    /// The chain's local child and entry counters under `lineage`. The
    /// root chain's entry 0 is the H1 pass (section 0), consumed whether
    /// or not the prompt has H1 blocks, so the first walked section is
    /// always `0.1`.
    counters: Counters,
    /// The nearest enclosing task: the chain's own id when the chain is a
    /// spawned task (a fanout arm included), its caller's task for a
    /// `call` child (a blocking child never interleaves with its caller,
    /// so the two share one task), and task `0` for the root chain. Every
    /// section the chain enters reads it as `sys.taskid`.
    task: TaskId,
    /// The chain that spawned this chain, when the chain is a task's
    /// backing chain: the task's owner, the only chain allowed to wait on,
    /// inspect, or cancel it. `None` for the root and a `call` child.
    owner: Option<ChainIndex>,
    /// A spawned chain's `item` and `sys.index` seeds, consumed by its
    /// first section entry; `None` afterward and on every other chain.
    seed: Option<TaskSeed>,
    /// The tasks the chain is parked on in a `when_any` wait (or the
    /// model's `await_tasks`); empty while the chain is not waiting. A
    /// member's chain end delivers it and clears the set.
    waiting_on: Vec<TaskId>,
    /// The model's `await_tasks` call the chain is parked in, when
    /// `waiting_on` is that call's set rather than an author `when_any`:
    /// the member's end answers the model's tool call with the drained
    /// notices instead of delivering the member to the shim. `None`
    /// otherwise.
    awaiting: Option<AwaitTasks>,
    /// What the chain's suspended request is parked on, as `tasks.status`
    /// reports it (`chat`, `tool_call`, `user_input`, `store`, `timer`,
    /// `tasks`, `call`): set at dispatch, cleared when the answer resumes
    /// the chain. `None` while the chain runs or between blocks.
    blocked: Option<&'static str>,
    /// Model-task notices not yet delivered into the chain's next model
    /// round, in arrival order: queued when a model task the chain owns
    /// ends, drained by the loop shim's per-round request or by the
    /// model's `await_tasks` answer. The H1 hand-off moves them to the
    /// walk with the pass's tasks.
    task_notices: Vec<String>,
    /// The latest progress note published through `tasks.note` for the
    /// task this chain backs, reported by `tasks.status`.
    note: Option<String>,
    /// The chain's fork of the run context: the run's own for the root
    /// chain, `with_args` for a call chain's input override.
    ctx: RunState,
    /// The chain's VFS access capability, installed into each section VM
    /// the chain enters: the walk and the live H1 pass acquire their own,
    /// a call chain borrows its parent's (a blocking child is the same
    /// serial thread - no new identity, no false conflicts), and a task
    /// chain spawns its own from its spawner's. `None` only after the
    /// chain ends: the arena is append-only, so `finish` and
    /// `abort_subtree` take the slot to release the identity's claims at
    /// chain end rather than at scheduler drop - a fanout caller's merge
    /// after its arms are delivered must not meet a finished arm's
    /// lingering claims.
    access: Option<Arc<Access>>,
    /// The per-section frame (VM, `sys`, conversation, counts): `Some`
    /// while a section is entered, `None` before the first entry and
    /// between sections.
    frame: Option<SectionContext>,
    /// The sibling slice the chain walks, named by its path in the prompt
    /// tree the run shares. A jump to a child swaps this to the jumper's
    /// child slice until the child level exhausts.
    slice: SlicePath,
    /// The section of `slice` the chain is running, or the next entry
    /// candidate while the chain is between sections.
    index: usize,
    /// The suspended parent positions of the chain's jump-started child
    /// walks: the parent slice plus the jumper's index in it. A jump to a
    /// child pushes the current position and descends; when the child
    /// level exhausts, the pop resumes the parent after the jumper.
    positions: Vec<(SlicePath, usize)>,
    /// The section's in-flight or next Lua/prose block: while `coroutine`
    /// is `Some` this is the suspended block's index, otherwise the next
    /// block to start.
    block: usize,
    /// The coroutine handle for the in-flight Lua block: exists only while
    /// a block is running or suspended; a block that returns disposes of it.
    coroutine: Option<Thread>,
    /// The answer delivered for a suspended coroutine, consumed at resume.
    incoming: Option<Answer<Error>>,
    /// The pending Markdown buffer: the prose block the next Lua fence
    /// consumes, installed as that block's lazy `prose` template when the
    /// coroutine starts. Cleared at every section entry; an unconsumed
    /// buffer drops with the section, never evaluated.
    pending_prose: Option<String>,
    /// The walk's clipboard: seeds each section's VM at entry; the
    /// section's final `var` is read back before teardown and replaces the
    /// slot. A call chain's slot seeds from the caller's snapshot and
    /// is discarded with the chain, so the caller never sees the chain's
    /// writes.
    var: serde_json::Value,
    /// The chain's call nesting depth: each call child and each spawned
    /// task runs one level deeper. The recursion cap checks this field,
    /// never the chain-stack length - task chains live on the ready queue,
    /// not the stack, so only the field carries the accounting across a
    /// spawn boundary.
    call_depth: usize,
    /// The call parent blocked on this chain, if any.
    parent: Option<ChainIndex>,
    /// The tool scope the chain's last `chat` round advertised, keyed by
    /// alias: the round's answer is checked against it, so a tool name the
    /// model invents or reaches for outside the scope fails as out of
    /// scope. `None` before the chain's first round.
    advertised: Option<BTreeMap<String, DispatchTarget>>,
    /// The H1 marker: the chain runs the prompt's H1 blocks under its
    /// title - section 0. Such a chain runs the walk's rules with three
    /// deltas: the frame keeps id 0 (no section observations fire), a
    /// scalar return short-circuits the whole run, and the pass's end
    /// starts the root walk with the H1 `var` hand-off. The `slice`/`index`
    /// walk position stays at the top-level slice, unused until a jump out
    /// of H1 starts the walk at the resolved target.
    h1: bool,
}

impl Chain {
    /// The chain's access capability for section-VM installation. A live
    /// chain always holds one; `finish` and `abort_subtree` take it at
    /// chain end.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the chain's capability is gone,
    /// which only the chain-end paths do - a live chain always holds it.
    fn access(&self) -> Result<&Arc<Access>> {
        self.access
            .as_ref()
            .ok_or(Error::internal("a live chain holds its access capability"))
    }

    /// The chain's current section within `prompt`: the section at its
    /// walk position. Resolved against the caller's handle on the tree so
    /// the result outlives a mutable borrow of the chain.
    fn section<'p>(&self, prompt: &'p Prompt) -> &'p Section {
        &self.slice.resolve(prompt)[self.index]
    }

    /// The chain's current block sequence: the H1 pass's blocks, or
    /// the current section's blocks on the walk.
    fn blocks<'p>(&self, prompt: &'p Prompt) -> &'p [Block] {
        if self.h1 {
            prompt.h1_blocks()
        } else {
            self.section(prompt).blocks()
        }
    }

    /// The chain's current section name for observations and errors: the
    /// prompt's title for the live H1 pass, the section's name on the walk.
    fn section_name(&self) -> &str {
        let prompt = self.ctx.prompt();
        if self.h1 {
            prompt.title()
        } else {
            self.section(prompt).name()
        }
    }
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
    /// The call-nesting chain stack (LIFO): a call dispatch pushes
    /// the child, the child's finish pops it.
    stack: Vec<ChainIndex>,
    /// Chains eligible to resume (FIFO); the driver drains it before
    /// awaiting anything.
    ready: VecDeque<ChainIndex>,
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
    /// The effects whose chain stopped waiting before the host answered (a
    /// chain end, a task cancel or abandonment, the run's teardown): the
    /// host still owes each one answer, which is discarded on arrival. An
    /// unknown id that is neither pending nor orphaned means the host
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
}

impl Scheduler {
    /// Builds the scheduler for one run over `ctx`'s prompt and reports
    /// the run's start, so the first step's events open with it.
    pub(crate) fn new(ctx: RunState) -> Self {
        // The run's boundaries are events like every other report: pushed
        // into the buffer under the root task, so the host sees them in
        // order with the sections between them.
        ctx.emitter()
            .report(ctx.prompt().title(), detail::RUN_STARTED);
        Self {
            ctx,
            chains: Vec::new(),
            stack: Vec::new(),
            ready: VecDeque::new(),
            pending: HashMap::new(),
            tasks: HashMap::new(),
            issued: Vec::new(),
            orphaned: HashSet::new(),
            phase: Phase::Fresh,
            max_chains: u32::MAX as usize,
            next_effect: 0,
        }
    }

    /// The prompt's shared handle: a chain resolves its walk position
    /// against this clone so the tree borrow never pins the arena.
    fn prompt(&self) -> Arc<Prompt> {
        Arc::clone(self.ctx.prompt_arc())
    }

    /// The run's context.
    pub(crate) fn state(&self) -> &RunState {
        &self.ctx
    }

    /// Whether the run's outcome is decided: the end boundary is reported
    /// and only the orphans' answers stand between the run and `Done`.
    pub(crate) fn decided(&self) -> bool {
        matches!(self.phase, Phase::Ending(_) | Phase::Done)
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
}
