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
//! pending table while a spawned task runs the single gateway round and
//! posts the answer to the channel; a chain that yields a structural
//! request (`call`) blocks while its child chain runs, and the child's
//! finish delivers its final text as the parent's answer. When no chain is
//! ready the driver awaits the answer channel or cancellation, whichever
//! comes first.
//!
//! [`RunState`] stays the ambient shared read-mostly context, borrowed by
//! chain steps; the scheduler is the exclusively owned mutable counterpart.
//! The two are deliberately not merged: `RunState` is cloned into
//! callbacks, while the scheduler must stay unreachable from the callback
//! layer.
//!
//! This file carries the scheduler core: the chain record and arena, the
//! ready queue, the pending table, the call stack, and the driver loop.
//! The submodules carry the rest: `chain` the chain lifecycle (arena
//! insertion and the two chain-end paths), `step` one chain's step to its
//! next suspension point, `walk` the section walk rules, `dispatch` the
//! request arms, `chat` the one-round `chat` arm and its answer
//! application, `tool_call` the script and model-issued `tool_call` arm
//! (the two arms the section-visible `models.loop` shim drives), and
//! `tasks` the fanout join tables and arm bookkeeping.

mod chain;
mod chat;
mod dispatch;
mod step;
mod tasks;
mod tool_call;
mod walk;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use mlua::Thread;
use shared_vfs::Origin;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::client::{Completion, GatewayClient};
use crate::parser::{Block, Prompt, Section};
use crate::store::Access;
use crate::{Error, Result, cancel};

use super::context::RunState;
use super::gateway::GatewaySource;
use super::protocol::Answer;
use super::scope::DispatchTarget;
use super::section_context::SectionContext;
use super::support::GENERIC_COMPLETION;
use tasks::{ArmState, FanoutId, JoinState};

/// Run-global monotonic id of an in-flight leaf request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RequestId(u64);

/// What a spawned leaf task posts back for its request.
///
/// Most leaf work posts its finished [`Answer`]. A `chat` round posts the
/// raw completion instead: the round's events and the advertised-scope
/// check need the parked chain, so the driver classifies the completion
/// into the answer on its own thread when it applies it.
enum Arrival {
    /// A finished answer, resumed into the chain as delivered.
    Answer(Answer<Error>),
    /// One `chat` round's completion or failure, classified by the driver.
    /// Boxed so the body-carrying completion does not size every arrival.
    Chat(std::result::Result<Box<Completion>, Error>),
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

/// Arena index of a chain: ids, not references, so no chain ever holds a
/// pointer to another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ChainId(u32);

impl ChainId {
    /// The arena index as a `usize`.
    fn index(self) -> usize {
        self.0 as usize
    }
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
    /// The chain's fork of the run context: the run's own for the root
    /// chain, `with_args` for a call chain's input override.
    ctx: RunState,
    /// The chain's VFS access capability, installed into each section VM
    /// the chain enters: the walk and the live H1 pass acquire their own,
    /// a call chain borrows its parent's (a blocking child is the same
    /// serial thread - no new identity, no false conflicts), and a fanout
    /// arm spawns its own from the fanout caller's. `None` only after the
    /// chain ends: the arena is append-only, so `finish` and
    /// `abort_subtree` take the slot to release the identity's claims at
    /// chain end rather than at scheduler drop - a fanout's join merge
    /// must not meet a finished arm's lingering claims.
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
    /// The chain's call nesting depth: each call child runs one level
    /// deeper. The recursion cap checks this field, never the chain-stack
    /// length - fanout arms live on the ready queue, not the stack, so only
    /// the field carries the accounting across a fanout boundary.
    call_depth: usize,
    /// The chain's client slot: seeded from the parent, resolved lazily on
    /// first inference through the scheduler's gateway source, so a
    /// construction error surfaces at first use rather than being swallowed.
    client: Option<GatewayClient>,
    /// The call parent blocked on this chain, if any.
    parent: Option<ChainId>,
    /// The tool scope the chain's last `chat` round advertised, keyed by
    /// alias: the round's answer is checked against it, so a tool name the
    /// model invents or reaches for outside the scope fails as out of
    /// scope. `None` before the chain's first round.
    advertised: Option<BTreeMap<String, DispatchTarget>>,
    /// The fanout-arm state when this chain is a fanout arm: the arm runs
    /// the same walk machinery as any chain, and its finish writes its
    /// join's result slot instead of a call answer.
    arm: Option<ArmState<'a>>,
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
/// table, join table, and answer channel, owned outright by the driver
/// loop's stack frame.
pub(crate) struct Scheduler<'a> {
    /// The ambient run context, borrowed by chain steps and forked by
    /// call chains.
    ctx: &'a RunState,
    /// The chain arena: append-only, indexed by [`ChainId`].
    chains: Vec<Chain<'a>>,
    /// The call-nesting chain stack (LIFO): a call dispatch pushes
    /// the child, the child's finish pops it.
    stack: Vec<ChainId>,
    /// Chains eligible to resume (FIFO); the driver drains it before
    /// awaiting anything.
    ready: VecDeque<ChainId>,
    /// One entry per in-flight leaf request, mapping it to the parked chain.
    pending: HashMap<RequestId, ChainId>,
    /// One join state per live fanout.
    joins: HashMap<FanoutId, JoinState<'a>>,
    /// The send half every spawned leaf task posts its answer to. The
    /// channel is unbounded: each task sends exactly once, and the in-flight
    /// count is already bounded by the chains that produced them.
    answer_tx: mpsc::UnboundedSender<(RequestId, Arrival)>,
    /// The receive half the driver awaits when no chain is ready.
    answers: mpsc::UnboundedReceiver<(RequestId, Arrival)>,
    /// Join handles of the in-flight leaf I/O tasks, keyed by request so
    /// a fatal fanout arm can abort a sibling arm's own in-flight round;
    /// every handle is aborted on cancellation or on the driver future's
    /// drop, and aborting a completed task is a no-op. The handles are
    /// kept joinable (not bare abort handles) so a terminal run outcome
    /// can drain them: a store op runs on the blocking pool, where abort
    /// detaches rather than interrupts, and only the op's completion
    /// drops its access clone.
    io_tasks: HashMap<RequestId, JoinHandle<()>>,
    /// The request ids whose in-flight tasks an abort discarded: a task
    /// that posted its answer before the abort landed delivers it late,
    /// and the driver discards exactly those answers. An unknown id that
    /// was never aborted means the driver dropped a pending entry early -
    /// answer loss that fails loudly rather than passing silently. An id
    /// leaves the set when its late answer arrives, so the set stays
    /// bounded by the aborts whose answers have not landed.
    aborted_requests: HashSet<RequestId>,
    /// The most chains one run may start: the arena indexes chains by
    /// `u32`, so the count is bounded by the index space. A field rather
    /// than a constant so a test can shrink the bound and drive the
    /// overflow path without allocating the real one.
    max_chains: usize,
    /// The next leaf-request id.
    next_request: u64,
    /// The next fanout id.
    next_fanout: u32,
    /// The run's gateway source: chains resolve their client slot through
    /// it on first inference.
    client: GatewaySource,
}

/// Aborts every in-flight leaf task when the driver future is dropped
/// mid-suspension - a host tearing the run down without polling it to a
/// terminal state. Dropping a bare `JoinHandle` detaches the task, which
/// would strand a broker wait or gateway round forever (a session close
/// would leak its pending input wait and never emit `input_cancelled`),
/// so the drop path applies the same abort the cancellation path does.
/// The claims-release join in [`Self::drain_io_tasks`] is unnecessary
/// here: a dropped run delivers no result.
impl Drop for Scheduler<'_> {
    fn drop(&mut self) {
        for handle in self.io_tasks.values() {
            handle.abort();
        }
    }
}

impl<'a> Scheduler<'a> {
    /// Builds the scheduler for one run over `ctx`'s prompt. `client` is the
    /// run's gateway client, if the caller supplied one; otherwise each
    /// chain builds one from the environment on first inference.
    pub(crate) fn new(ctx: &'a RunState, client: Option<GatewayClient>) -> Self {
        let (answer_tx, answers) = mpsc::unbounded_channel();
        Self {
            ctx,
            chains: Vec::new(),
            stack: Vec::new(),
            ready: VecDeque::new(),
            pending: HashMap::new(),
            joins: HashMap::new(),
            answer_tx,
            answers,
            io_tasks: HashMap::new(),
            aborted_requests: HashSet::new(),
            max_chains: u32::MAX as usize,
            next_request: 0,
            next_fanout: 0,
            client: GatewaySource::from_optional(client, ctx.limits()),
        }
    }

    /// Shrinks the chain-count bound so a test can drive the
    /// [`start_chain`](Self::start_chain) overflow path.
    #[cfg(test)]
    pub(crate) fn set_max_chains_for_test(&mut self, limit: usize) {
        self.max_chains = limit;
    }

    /// The number of leaf requests the run has issued so far, so a test
    /// can prove a dispatch was answered inline with no spawned leaf work.
    #[cfg(test)]
    pub(crate) fn leaf_requests_issued(&self) -> u64 {
        self.next_request
    }

    /// Posts an answer for an arbitrary request id, so a test can drive
    /// the driver's unknown-answer paths directly.
    #[cfg(test)]
    pub(crate) fn post_answer_for_test(&self, request: u64, answer: Answer<Error>) {
        self.answer_tx
            .send((RequestId(request), Arrival::Answer(answer)))
            .expect("the scheduler holds its own receiver");
    }

    /// Drives the run until it ends and returns the run's result: the H1
    /// pass first when the prompt has H1 blocks, then the root chain over
    /// the prompt's sections.
    ///
    /// Leaf dispatch spawns plain tasks (not `spawn_local`): an infer task
    /// touches no scheduler state and no Lua value - it awaits one gateway
    /// round and posts the answer to the channel - so the driver future
    /// stays `Send` and a caller may spawn the run onto a multi-thread
    /// runtime. On a current-thread runtime the spawned tasks run on that
    /// one thread anyway.
    ///
    /// # Errors
    /// Returns the [`Error`] of whichever step failed: frame construction,
    /// a Lua block, or a dispatched request's answer.
    /// Returns [`Error::Interrupted`] when the run's cancellation handle is
    /// signaled while chains are running or suspended.
    pub(crate) async fn drive(&mut self) -> Result<String> {
        let result = self.drive_inner().await;
        self.drain_io_tasks().await;
        result
    }

    /// Claims-release ordering constraint: the run's result - success,
    /// determinism failure, or cancellation alike - must not be delivered
    /// while an in-flight leaf op still holds its access clone. A store
    /// op runs on the blocking pool, where aborting the task detaches
    /// rather than interrupts, so an abandoned op would release its
    /// identity's claims only when the closure finishes - past the run's
    /// end, where a fresh access could meet the lingering claim. Abort
    /// every task still recorded (prompt for an async task, a no-op for
    /// a blocking op already running, which runs to completion), then
    /// await each handle: the join resolves only once the op's access
    /// clone - and with it the identity's claims - is gone. This changes
    /// when claims release, never what an operation does.
    async fn drain_io_tasks(&mut self) {
        let tasks = std::mem::take(&mut self.io_tasks);
        for task in tasks.values() {
            task.abort();
        }
        for (_, task) in tasks {
            let _ = task.await;
        }
    }

    async fn drive_inner(&mut self) -> Result<String> {
        // The H1 pass runs when the prompt has H1 blocks; an H1-less prompt
        // goes straight to the walk, so its shared library never pays for a
        // throwaway section-0 replay.
        if self.ctx.prompt().h1_blocks().is_empty() {
            let sections = self.ctx.prompt().sections();
            if sections.is_empty() {
                return Ok(GENERIC_COMPLETION.to_owned());
            }
            self.start_root_walk(sections, &serde_json::json!({}))?;
        } else {
            let h1 = self.start_live_h1()?;
            self.ready.push_back(h1);
        }
        let mut root_result = None;
        loop {
            while let Some(id) = self.ready.pop_front() {
                // Cancellation between steps: the instruction hook covers
                // running Lua and the select below covers suspension, but a
                // run whose chains never suspend on I/O would otherwise
                // finish without ever observing the handle - the legacy
                // fanout driver's select loop observed it at arm
                // boundaries.
                if cancel::is_cancelled() {
                    return Err(Error::Interrupted);
                }
                if let Err(error) = self.step(id, &mut root_result).await {
                    self.finish(id, Err(error), &mut root_result);
                }
                if let Some(result) = root_result.take() {
                    return result;
                }
            }
            // Every unfinished chain is ready, pending on I/O, or blocked on
            // a child that transitively bottoms out in a ready or pending
            // chain, so an empty ready queue with an empty pending table can
            // only be a driver bug - fail loudly rather than hang.
            if self.pending.is_empty() {
                return Err(Error::internal(
                    "the scheduler stalled with no ready chain and no in-flight request",
                ));
            }
            tokio::select! {
                biased;
                // Cancellation while suspended: abort the in-flight leaf
                // tasks and fail the run. The suspended chains' frames drop
                // unarmed with the scheduler - the same outcome as the
                // hook-driven path while running - and each fanout arm's
                // finalizer drop reports its FANOUT_ARM_CANCELLED terminal
                // observation, so the exactly-once terminal contract holds
                // on this path too.
                () = cancel::wait_cancelled() => {
                    for handle in self.io_tasks.values() {
                        handle.abort();
                    }
                    return Err(Error::Interrupted);
                }
                arrival = self.answers.recv() => {
                    let Some((request_id, arrival)) = arrival else {
                        return Err(Error::internal(
                            "the answer channel cannot close while the scheduler holds its sender",
                        ));
                    };
                    self.io_tasks.remove(&request_id);
                    let Some(chain_id) = self.pending.remove(&request_id) else {
                        // A late answer from an I/O task whose chain was
                        // already aborted (a fatal sibling's fanout abort
                        // races a task that sent before the abort landed):
                        // the abort recorded the request id, so the answer
                        // is moot. Any other unknown id means the driver
                        // dropped a pending entry early - answer loss that
                        // must fail loudly, not pass silently.
                        if self.aborted_requests.remove(&request_id) {
                            continue;
                        }
                        return Err(Error::internal(
                            "an answer arrived for a request with no pending entry and no recorded abort",
                        ));
                    };
                    // A chat round's completion becomes its answer here, on
                    // the driver thread: the round's events fire against the
                    // chain's own reporting handles and the tool calls are
                    // checked against the scope the chain advertised.
                    let answer = match arrival {
                        Arrival::Answer(answer) => answer,
                        Arrival::Chat(result) => self.accept_chat(chain_id, result)?,
                    };
                    match answer {
                        // A claims-model conflict is fatal: the run ends on
                        // the spot with the determinism violation rather
                        // than resuming it into Lua, where an author
                        // `pcall` could catch it. The suspended chains drop
                        // unarmed with the scheduler, each fanout arm's
                        // finalizer reporting its cancelled terminal
                        // observation, exactly as on the cancellation path.
                        Answer::Store(Err(error @ Error::Determinism(_))) => return Err(error),
                        answer => {
                            self.chains[chain_id.index()].incoming = Some(answer);
                            self.ready.push_back(chain_id);
                        }
                    }
                }
            }
        }
    }
}
