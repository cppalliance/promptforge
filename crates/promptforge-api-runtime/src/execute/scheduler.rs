//! The chain-stack scheduler: the coroutine protocol's driver loop.
//!
//! One [`Scheduler`] per run, created and owned by the top-level run call
//! and living entirely in the driver loop's stack frame: no `Arc`, no
//! `Mutex`, no sharing. One thread runs every chain step, and the Lua shims
//! only yield (they never call into Rust for suspending operations), so the
//! scheduler state is unreachable from Lua. Leaf dispatch spawns plain
//! tasks: an infer task touches no scheduler state and no Lua value, so
//! the driver future stays `Send` and the run may be spawned onto a
//! multi-thread runtime; on a current-thread runtime the whole run stays
//! on its one thread.
//!
//! The loop is `resume -> match request -> dispatch -> resume with answer`.
//! A chain whose coroutine yields a leaf request (`infer`) is parked in the
//! pending table while its [`Effect`] is performed: the arm builds the
//! effect as a value and hands it to the internal performer table, which
//! spawns the leaf work and posts the [`EffectAnswer`] to the channel
//! under the effect's id; the driver applies the answer on its own thread
//! through one `apply_answer`, emitting the round's events there. A chain
//! that yields a structural request (`call`) blocks while its child chain
//! runs, and the child's finish delivers its final text as the parent's
//! answer. When no chain is ready the driver awaits the answer channel or
//! cancellation, whichever comes first.
//!
//! [`RunState`] stays the ambient shared read-mostly context, borrowed by
//! chain steps; the scheduler is the exclusively owned mutable counterpart.
//! The two are deliberately not merged: `RunState` is cloned into
//! callbacks, while the scheduler must stay unreachable from the callback
//! layer.
//!
//! This file carries the scheduler core: the chain record and arena, the
//! ready queue, the pending table, the call stack, the task arena, and
//! the one `issue` path every leaf arm hands its effect through. The
//! submodules carry the rest: `drive` the driver loop, `performers` the
//! internal performer table (today's spawned leaf work behind the effect
//! boundary), `apply` the answer application, `chain` the chain
//! lifecycle (arena insertion and the two chain-end paths), `step` one
//! chain's step to its next suspension point, `walk` the section walk
//! rules, `h1` the live H1 pass and its hand-off to the walk, `dispatch`
//! the request arms, `chat` the one-round `chat` arm and its answer
//! application, `tool_call` the script and model-issued `tool_call` arm
//! (the two arms the section-visible `models.loop` shim drives),
//! `builtins` the model's task built-ins (`task`, `task_cancel`,
//! `task_status`) answered over the arena and advertised once a section
//! runs `tools.allow_tasks`, `await_tasks` the fourth built-in, the
//! model's wait over its live tasks, `notices` the model-task notices
//! (queued at a model task's end, drained into the owner's next round or
//! its `await_tasks` answer), `tasks` the task arena, the `spawn` arm, and
//! the chain-end rules for tasks, `waits` the `when_any` wait and the
//! `ready`, `status`, `pending`, `note`, and `cancel` arms over the arena,
//! `timer` the wait shims' internal timeout as an effect-backed slot, and
//! `test_hooks` (test builds only) the seams the suites drive the
//! driver's edge paths through.
//! A fanout is Lua over those arms (the `fanout` shim spawns one task per
//! member and waits on the live set), so the scheduler keeps no fanout
//! state of its own.

mod apply;
mod await_tasks;
mod builtins;
mod chain;
mod chat;
mod dispatch;
mod drive;
mod h1;
mod notices;
mod performers;
mod step;
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
use promptforge_api_types::ids::{ChainId, TaskId};
use shared_vfs::Origin;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::client::GatewayClient;
use crate::lua::{ScriptReport, ToolBinding};
use crate::observe::Observation;
use crate::parser::{Block, Prompt, Section};
use crate::store::Access;
use crate::{Error, Result};

use super::context::RunState;
use super::protocol::Answer;
use super::run::{Effect, EffectAnswer, EffectId};
use super::scope::DispatchTarget;
use super::section_context::{SectionContext, TaskSeed};
use await_tasks::AwaitTasks;
use performers::Performers;
use tasks::TaskSlot;
#[cfg(test)]
pub(crate) use tasks::TaskState;

/// The driver-side half of one issued leaf effect: what the parked chain
/// asked for, in the terms `apply_answer` needs to turn the performer's
/// raw [`EffectAnswer`] into the chain's protocol [`Answer`] and emit the
/// round's events. The effect itself carries none of this: it describes
/// the work, this describes what the work means to the chain.
enum Continuation {
    /// A nested `models.infer`: the completion becomes the round's text
    /// under the single-prose-round reporting rules.
    Infer,
    /// A `chat` round: the completion is classified against the scope the
    /// chain advertised and reported as one model turn.
    Chat,
    /// A bound tool call: the tool's own output goes through the shared
    /// dispatch body (counts already taken at dispatch, then the
    /// succeeded/failed event, the trust rule, and the `ToolResult`).
    ToolCall(ToolCallContinuation),
    /// A `user_input` wait: the broker's text is reported and resumes with
    /// its availability flag.
    UserInput,
    /// A store operation: the succeeded/failed observation pair its
    /// outcome reports, `None` for `exists`, which reports nothing.
    Store(Option<(Observation, Observation)>),
    /// The internal timer behind a timed wait: the firing completes the
    /// slot backed by the effect and wakes its waiting owner; no chain
    /// resumes.
    Timer,
}

/// What a bound `tool_call`'s answer is applied with: the binding the call
/// resolved to (its alias, output kind, and trust rules), the coordinates
/// the `ToolResult` reports under, and the model's call id when the model
/// issued the call.
struct ToolCallContinuation {
    /// The binding the alias resolved to at dispatch.
    binding: ToolBinding,
    /// The chain, depth, and turn the call fired in.
    report: ScriptReport,
    /// The model-issued call id, or `None` for a script call.
    call_id: Option<String>,
}

/// One in-flight leaf effect's pending entry: the chain parked on it and
/// how its answer resumes that chain.
struct Pending {
    /// The parked chain (for a timer, the owner whose wait the timer
    /// serves).
    chain: ChainIndex,
    /// How the answer is applied.
    resume: Continuation,
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
struct Chain<'a> {
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
    /// The sibling slice the chain walks, borrowed from the prompt tree,
    /// which outlives the scheduler. A jump to a child swaps this to the
    /// jumper's child slice until the child level exhausts.
    slice: &'a [Section],
    /// The section of `slice` the chain is running, or the next entry
    /// candidate while the chain is between sections.
    index: usize,
    /// The suspended parent positions of the chain's jump-started child
    /// walks: the parent slice plus the jumper's index in it. A jump to a
    /// child pushes the current position and descends; when the child
    /// level exhausts, the pop resumes the parent after the jumper.
    positions: Vec<(&'a [Section], usize)>,
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
    /// The H1 marker: the prompt's H1 blocks under its title - section 0.
    /// `Some` chains run the walk's rules with three deltas: the frame
    /// keeps id 0 (no section observations fire), a scalar return
    /// short-circuits the whole run, and the pass's end starts the root
    /// walk with the H1 `var` hand-off. The `slice`/`index` walk position
    /// stays empty and unused until a jump out of H1 starts the walk at
    /// the resolved target.
    h1: Option<&'a [Block]>,
}

impl<'a> Chain<'a> {
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

    /// The chain's current block sequence: the H1 pass's blocks, or
    /// the current section's blocks on the walk.
    fn blocks(&self) -> &'a [Block] {
        match &self.h1 {
            Some(blocks) => blocks,
            None => self.slice[self.index].blocks(),
        }
    }

    /// The chain's current section name for observations and errors: the
    /// prompt's title for the live H1 pass, the section's name on the walk.
    fn section_name(&self) -> &str {
        match self.h1 {
            Some(_) => self.ctx.prompt().title(),
            None => self.slice[self.index].name(),
        }
    }
}

/// The coroutine protocol's driver: the chain arena, ready queue, pending
/// table, task arena, and answer channel, owned outright by the driver
/// loop's stack frame.
pub(crate) struct Scheduler<'a> {
    /// The ambient run context, borrowed by chain steps and forked by
    /// call chains.
    ctx: &'a RunState,
    /// The chain arena: append-only, indexed by [`ChainIndex`].
    chains: Vec<Chain<'a>>,
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
    /// The internal performer table: today's spawned leaf work behind the
    /// effect boundary. It holds the send half of the answer channel and
    /// posts every answer under its effect's id.
    performers: Performers<'a>,
    /// The receive half the driver awaits when no chain is ready.
    answers: mpsc::UnboundedReceiver<(EffectId, EffectAnswer)>,
    /// Join handles of the in-flight leaf I/O tasks, keyed by effect so
    /// a cancelled task chain's own in-flight round can be aborted with
    /// it; every handle is aborted on cancellation or on the driver future's
    /// drop, and aborting a completed task is a no-op. The handles are
    /// kept joinable (not bare abort handles) so a terminal run outcome
    /// can drain them: a store op runs on the blocking pool, where abort
    /// detaches rather than interrupts, and only the op's completion
    /// drops its access clone.
    io_tasks: HashMap<EffectId, JoinHandle<()>>,
    /// The effect ids whose in-flight tasks an abort discarded (a chain
    /// end or a `Dropped` answer): a task that posted its answer before
    /// the abort landed delivers it late, and the driver discards exactly
    /// those answers. An unknown id that was never aborted means the
    /// driver dropped a pending entry early - answer loss that fails
    /// loudly rather than passing silently. An id leaves the set when its
    /// late answer arrives, so the set stays bounded by the aborts whose
    /// answers have not landed.
    aborted_effects: HashSet<EffectId>,
    /// The most chains one run may start: the arena indexes chains by
    /// `u32`, so the count is bounded by the index space. A field rather
    /// than a constant so a test can shrink the bound and drive the
    /// overflow path without allocating the real one.
    max_chains: usize,
    /// The next effect id: a run-wide counter, so every effect the run
    /// issues has a distinct in-flight handle.
    next_effect: u64,
}

impl<'a> Scheduler<'a> {
    /// Builds the scheduler for one run over `ctx`'s prompt. `client` is the
    /// run's gateway client, if the caller supplied one; otherwise the run
    /// builds one from the environment on first inference.
    pub(crate) fn new(ctx: &'a RunState, client: Option<GatewayClient>) -> Self {
        let (performers, answers) = Performers::new(ctx, client);
        Self {
            ctx,
            chains: Vec::new(),
            stack: Vec::new(),
            ready: VecDeque::new(),
            pending: HashMap::new(),
            tasks: HashMap::new(),
            performers,
            answers,
            io_tasks: HashMap::new(),
            aborted_effects: HashSet::new(),
            max_chains: u32::MAX as usize,
            next_effect: 0,
        }
    }

    /// Issues one leaf effect for `chain`: allocates its id, hands it to
    /// the performer table (which spawns the leaf work and will post the
    /// answer under the id), and parks the chain in the pending table with
    /// `resume`, the rule its answer is applied by. The one path every
    /// leaf arm takes, so no arm spawns or parks on its own.
    ///
    /// # Errors
    /// Returns the performer's error when the effect cannot be performed
    /// (the gateway client cannot be built, or the tool the effect names
    /// is not in the run's catalog). The id is consumed either way.
    fn issue(
        &mut self,
        chain: ChainIndex,
        effect: Effect,
        resume: Continuation,
    ) -> Result<EffectId> {
        let id = EffectId(self.next_effect);
        self.next_effect += 1;
        let task = self.performers.perform(id, effect)?;
        self.io_tasks.insert(id, task);
        self.pending.insert(id, Pending { chain, resume });
        Ok(id)
    }
}
