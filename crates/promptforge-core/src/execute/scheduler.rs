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
//! request (`execute`) blocks while its child chain runs, and the child's
//! finish delivers its final text as the parent's answer. When no chain is
//! ready the driver awaits the answer channel or cancellation, whichever
//! comes first.
//!
//! [`RunContext`] stays the ambient shared read-mostly context, borrowed by
//! chain steps; the scheduler is the exclusively owned mutable counterpart.
//! The two are deliberately not merged: `RunContext` is cloned into
//! callbacks, while the scheduler must stay unreachable from the callback
//! layer.
//!
//! This module carries the scheduler core plus the walk rules: sections run
//! in fall-through order, a section marked off-walk is skipped unless the
//! arrival is addressed (a jump or execute target runs anyway), the reply
//! and `var` roll forward across sections and jumps, every section entry
//! takes the next run-global id, and a jump transfers control - a sibling
//! move within the chain's slice, or a descent into the jumper's child
//! slice with the parent position suspended on the chain's own position
//! stack until the child level exhausts. A drive armed with
//! [`Scheduler::with_live_h1`] runs the live H1 pass first: the prompt's
//! H1 blocks as the driver loop's first chain, under the live pass's rules
//! (id 0, no section observations, a jump is an error, a scalar return
//! short-circuits the run), with the root walk starting from the H1 `var`
//! hand-off. A `fanout` request forks N arm chains (one per collection
//! member) interleaved by the driver: at most the run's
//! `max_fanout_concurrency` arms are active at once, each arm runs the
//! same walk machinery as any chain over the worker's blocks, and the join
//! state's preallocated per-index slots deliver the results to the parent
//! in collection order, never finish order. The fanout failure semantics
//! match the legacy engine: an empty collection errors before any
//! scheduling, a fatal arm error aborts the sibling arms (each aborted
//! arm's finalizer reports `FANOUT_ARM_CANCELLED`, so exactly one terminal
//! observation fires per arm), [`Error::ToolLoopExhausted`] soft-degrades
//! its arm to the incomplete stub, and two arms of one fanout writing the
//! same store path fail with the store's write-write race error while
//! `append` stays legal with unspecified order. A received `mcp` request
//! is the protocol's typed reserved error.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use mlua::Thread;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::client::GatewayClient;
use crate::fanout;
use crate::fanout::ArmFinalizer;
use crate::lua::{
    CoroStep, LuaBlockResult, LuaFanoutResult, LuaProgram, SectionVm, resolve_model_binding,
    shim_live_h1_models,
};
use crate::model::ModelBinding;
use crate::observe::detail;
use crate::parser::{Block, Section};
use crate::resolve::RuntimeResolution;
use crate::{Error, Result, cancel, subst};

use super::context::RunContext;
use super::engine::{
    JumpTarget, home_without, resolve_jump_target, section_position, visible_sections,
};
use super::gateway::{GatewaySource, ResolutionContext};
use super::protocol::{Answer, Request, YieldParse};
use super::section_context::SectionContext;
use super::support::{GENERIC_COMPLETION, MAX_EXECUTE_DEPTH, next_id, now_rfc3339_checked};
use super::tools::infer_round;

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

/// Run-global monotonic id of an in-flight leaf request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RequestId(u64);

/// Join-table key for a live fanout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FanoutId(u32);

/// One live fanout's join state: the arms still running, the preallocated
/// per-index result slots (collection order, never finish order), the
/// parent chain blocked on the join, the concurrency-window accounting, and
/// the template every arm chain starts from.
struct JoinState<'a> {
    /// Arms still running; at zero the parent resumes with the sequence.
    remaining: usize,
    /// One slot per collection index, so results land in collection order.
    results: Vec<Option<LuaFanoutResult>>,
    /// The chain that yielded the fanout, blocked until the join completes.
    parent: ChainId,
    /// Arms currently active (unblocked or pending on I/O); bounded by
    /// `window`.
    active: usize,
    /// The next collection index to start when a window slot frees.
    next: usize,
    /// The converted collection members, indexed by arm.
    items: Vec<serde_json::Value>,
    /// At most this many arms active at once: the run's
    /// `max_fanout_concurrency`.
    window: usize,
    /// Everything an arm chain starts from, shared by every arm of the
    /// fanout.
    template: ArmTemplate<'a>,
}

/// The arm-chain construction inputs one fanout's arms share.
#[derive(Clone)]
struct ArmTemplate<'a> {
    /// The fanout caller's walk position: an arm's visible set derives from
    /// it (the caller's slice minus the caller, plus the caller's children,
    /// minus the worker, plus the worker's children).
    caller_slice: &'a [Section],
    /// The caller's index in `caller_slice`.
    caller_index: usize,
    /// The slice the worker was resolved from (the caller's own slice or
    /// the caller's children).
    worker_slice: &'a [Section],
    /// The worker's index in `worker_slice`.
    worker_index: usize,
    /// The fanout's run-context fork: the run's own observer and debug sink
    /// (the legacy proxies exist to cross the spawned-task boundary, which
    /// a chain never crosses) with a fresh turn counter, so arm turns count
    /// against the fanout's own cap.
    ctx: RunContext,
    /// This fanout's store-write token: one per fanout, so the store's
    /// write registry can tell two arms of this fanout (a write-write race)
    /// from a later fanout's write (legal).
    write_token: u64,
    /// The caller's reply seed for every arm's roll-forward.
    reply: Option<String>,
    /// The caller's `var` snapshot; each arm seeds from its own clone and
    /// its writes never reach the caller.
    var: serde_json::Value,
    /// The arms' execute depth: the fanout caller's depth plus one.
    execute_depth: usize,
    /// The caller's client snapshot: each arm starts from it, resolving one
    /// lazily when absent.
    client: Option<GatewayClient>,
    /// The run's cancellation handle, captured at dispatch and handed to
    /// each arm chain directly (the scheduler has no spawned arm tasks to
    /// carry one across).
    cancel: Option<cancel::CancelHandle>,
}

/// One arm chain's fanout state: where its result lands and what its
/// worker entry is seeded with.
struct ArmState<'a> {
    /// The join this arm reports to.
    fanout: FanoutId,
    /// The arm's 0-based collection index: its result slot and (plus one)
    /// its `sys.index` and write-scope arm id.
    item_index: usize,
    /// The arm's collection member: the `item` global and `{{ item }}`
    /// substitution seed for the worker entry.
    item: serde_json::Value,
    /// This fanout's store-write token.
    write_token: u64,
    /// True while the worker is the chain's current section: the worker
    /// entry gets the arm seeds, and a control transfer out of the worker
    /// resolves over the arm's visible set. Cleared by the first jump.
    at_worker: bool,
    /// The fanout caller's walk position, as on the template.
    caller_slice: &'a [Section],
    /// The caller's index in `caller_slice`.
    caller_index: usize,
    /// The resolved worker's home slice position.
    worker_slice: &'a [Section],
    /// The worker's index in `worker_slice`.
    worker_index: usize,
    /// The run's cancellation handle, installed as the task-local around
    /// each of the arm's steps, so cancellation reaches the arm's running
    /// Lua through its instruction hook exactly as on the driver task.
    cancel: Option<cancel::CancelHandle>,
    /// The arm's terminal-observation guard: the driver finishes it with
    /// the arm's real outcome (succeeded, exhausted, or failed), and its
    /// drop reports `FANOUT_ARM_CANCELLED` - a sibling's fatal error
    /// aborting this arm, or the run's cancellation dropping the
    /// scheduler, both pass through that drop. Exactly one terminal event
    /// fires per arm (the legacy `ArmFinalizer` contract).
    finalizer: ArmFinalizer,
}

/// A heading resolved against a chain's visible set: the slice the walk or
/// a contained chain continues on, the target's index in it, and whether
/// the target is a direct child of the current section (a descent).
struct ChainTarget<'a> {
    /// The slice the walk or chain continues on.
    slice: &'a [Section],
    /// The target's index in `slice`.
    index: usize,
    /// True when the target is a direct child of the current section.
    child: bool,
}

/// Resolves `heading` against an at-worker arm's visible set: the fanout
/// caller's visible set minus the worker, plus the worker's children - the
/// set the legacy arm's control globals resolve over, built with the same
/// helpers so resolution and its error listings match exactly.
///
/// A sibling-level target walks its own prompt slice from its index: the
/// worker's home slice when it lives there, else the caller's children or
/// the caller's slice. The resolved `(level, name)` pair is unique across
/// the visible set (an ambiguous resolve already failed), so at most one
/// slice contains it. One legacy edge narrows here: the legacy arm walks
/// its materialized home slice (the caller's slice minus the caller and
/// the worker, concatenated with the caller's children), so a target that
/// precedes the worker never falls through back into it, and a
/// level-matched member of the caller's sibling slice walks on into the
/// caller's children; the scheduler walks the target's own prompt slice
/// instead.
///
/// # Errors
/// Returns [`Error::Lua`] when the heading is malformed, matches no
/// visible section, or matches more than one (see
/// [`fanout::resolve_sibling`]); [`Error::Internal`] when a resolved
/// target is absent from every home slice (an invariant violation).
fn resolve_arm_target<'a>(
    caller_slice: &'a [Section],
    caller_index: usize,
    worker_slice: &'a [Section],
    worker_index: usize,
    heading: &str,
) -> Result<ChainTarget<'a>> {
    let caller = &caller_slice[caller_index];
    let worker = &worker_slice[worker_index];
    let mut visible = home_without(&visible_sections(caller_slice, caller), worker);
    visible.extend(worker.children.iter().cloned());
    let target = fanout::resolve_sibling(heading, &visible)?;
    if let Some(index) = section_position(&worker.children, target) {
        return Ok(ChainTarget {
            slice: &worker.children,
            index,
            child: true,
        });
    }
    for slice in [worker_slice, caller.children.as_slice(), caller_slice] {
        if let Some(index) = section_position(slice, target) {
            return Ok(ChainTarget {
                slice,
                index,
                child: false,
            });
        }
    }
    Err(Error::Internal(
        "a resolved arm target is absent from its home slices",
    ))
}

/// One chain: a contained line of section execution, the scheduler's
/// counterpart to the legacy `walk_siblings` invocation.
///
/// The chain owns its per-section frame and adds the chain position (the
/// sibling slice being walked plus the current index), the coroutine handle
/// for the in-flight Lua block, and the walk-scoped slots: the reply rolled
/// forward across sections and the `var` clipboard. One section entry is
/// one frame; the fall-through advance tears the old frame down and the
/// next entry constructs the next.
struct Chain<'a> {
    /// The chain's fork of the run context: the run's own for the root
    /// chain, `with_args` for an execute chain's input override.
    ctx: RunContext,
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
    /// Set when the next entry is an addressed arrival (an execute target):
    /// an addressed section runs even when marked off-walk. One entry
    /// consumes the flag; fall-through arrival is never addressed.
    addressed: bool,
    /// The section's in-flight or next Lua/prose block: while `coroutine`
    /// is `Some` this is the suspended block's index, otherwise the next
    /// block to start.
    block: usize,
    /// The coroutine handle for the in-flight Lua block: exists only while
    /// a block is running or suspended; a block that returns disposes of it.
    coroutine: Option<Thread>,
    /// The answer delivered for a suspended coroutine, consumed at resume.
    incoming: Option<Answer>,
    /// The walk-scoped reply slot: seeds each section's frame at entry and
    /// is replaced by the section's final reply at its end, so the reply
    /// crosses section boundaries.
    reply: Option<String>,
    /// The walk's clipboard: seeds each section's VM at entry; the
    /// section's final `var` is read back before teardown and replaces the
    /// slot. An execute chain's slot seeds from the caller's snapshot and
    /// is discarded with the chain, so the caller never sees the chain's
    /// writes.
    var: serde_json::Value,
    /// The chain's execute nesting depth: each execute child runs one level
    /// deeper. The recursion cap checks this field, never the chain-stack
    /// length - fanout arms live on the ready queue, not the stack, so only
    /// the field carries the accounting across a fanout boundary.
    execute_depth: usize,
    /// The chain's client slot: seeded from the parent, resolved lazily on
    /// first inference through the scheduler's gateway source, so a
    /// construction error surfaces at first use rather than being swallowed.
    client: Option<GatewayClient>,
    /// The execute parent blocked on this chain, if any.
    parent: Option<ChainId>,
    /// The fanout-arm state when this chain is a fanout arm: the arm runs
    /// the same walk machinery as any chain, and its finish writes its
    /// join's result slot instead of an execute answer.
    arm: Option<ArmState<'a>>,
    /// The live H1 pass marker: the prompt's H1 blocks under its title.
    /// `Some` chains run the live pass's rules instead of the walk's: the
    /// frame keeps id 0, no section observations fire, a recorded jump is
    /// an error, a scalar return short-circuits the whole run, and the
    /// pass's end starts the root walk with the H1 `var` hand-off. The
    /// `slice`/`index` walk position stays empty and unused.
    h1: Option<&'a [Block]>,
}

impl Chain<'_> {
    /// The chain's current block sequence: the live H1 pass's blocks, or
    /// the current section's blocks on the walk.
    fn blocks(&self) -> &[Block] {
        match &self.h1 {
            Some(blocks) => blocks,
            None => &self.slice[self.index].blocks,
        }
    }

    /// The chain's current section name for observations and errors: the
    /// prompt's title for the live H1 pass, the section's name on the walk.
    fn section_name(&self) -> &str {
        match self.h1 {
            Some(_) => &self.ctx.prompt().title,
            None => &self.slice[self.index].name,
        }
    }
}

/// The coroutine protocol's driver: the chain arena, ready queue, pending
/// table, join table, and answer channel, owned outright by the driver
/// loop's stack frame.
pub(crate) struct Scheduler<'a> {
    /// The ambient run context, borrowed by chain steps and forked by
    /// execute chains.
    ctx: &'a RunContext,
    /// The chain arena: append-only, indexed by [`ChainId`].
    chains: Vec<Chain<'a>>,
    /// The execute-nesting chain stack (LIFO): an execute dispatch pushes
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
    answer_tx: mpsc::UnboundedSender<(RequestId, Answer)>,
    /// The receive half the driver awaits when no chain is ready.
    answers: mpsc::UnboundedReceiver<(RequestId, Answer)>,
    /// Abort handles of the in-flight leaf I/O tasks, keyed by request so
    /// a fatal fanout arm can abort a sibling arm's own in-flight round;
    /// every handle is aborted on cancellation, and aborting a completed
    /// task is a no-op.
    io_tasks: HashMap<RequestId, AbortHandle>,
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
    /// The live H1 pass's run-scoped capability resolution: `Some` when the
    /// scheduler runs the H1 pass before the walk (the run's shape),
    /// `None` for a walk-only drive whose shared sets were filled another
    /// way. One resolution serves the whole pass, so its decision cache
    /// keeps the single-flight guarantee across blocks and resumes.
    h1_resolution: Option<RuntimeResolution<'a>>,
}

impl<'a> Scheduler<'a> {
    /// Builds the scheduler for one run over `ctx`'s prompt. `client` is the
    /// run's gateway client, if the caller supplied one; otherwise each
    /// chain builds one from the environment on first inference.
    pub(crate) fn new(ctx: &'a RunContext, client: Option<GatewayClient>) -> Self {
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
            h1_resolution: None,
        }
    }

    /// Arms the live H1 pass: the drive runs the prompt's H1 blocks as the
    /// first chain, under the live pass's rules, before the root walk
    /// starts from the H1 hand-off. `resolution` carries the run's live
    /// picker and catalogs; the pass's binds write the run's shared sets,
    /// which the walk reads through the context's views.
    #[must_use]
    pub(crate) fn with_live_h1(mut self, resolution: ResolutionContext<'a>) -> Self {
        self.h1_resolution = Some(RuntimeResolution::new(
            resolution.picker,
            resolution.tools,
            resolution.models,
            self.ctx.tool_set(),
            self.ctx.model_set(),
        ));
        self
    }

    /// Shrinks the chain-count bound so a test can drive the
    /// [`start_chain`](Self::start_chain) overflow path.
    #[cfg(test)]
    pub(crate) fn set_max_chains_for_test(&mut self, limit: usize) {
        self.max_chains = limit;
    }

    /// Posts an answer for an arbitrary request id, so a test can drive
    /// the driver's unknown-answer paths directly.
    #[cfg(test)]
    pub(crate) fn post_answer_for_test(&self, request: u64, answer: Answer) {
        self.answer_tx
            .send((RequestId(request), answer))
            .expect("the scheduler holds its own receiver");
    }

    /// Drives the run until it ends and returns the run's result: the live
    /// H1 pass first when the scheduler was armed with
    /// [`with_live_h1`](Self::with_live_h1), then the root chain over the
    /// prompt's sections.
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
    /// a Lua block, prose inference, or a dispatched request's answer.
    /// Returns [`Error::Interrupted`] when the run's cancellation handle is
    /// signaled while chains are running or suspended.
    pub(crate) async fn drive(&mut self) -> Result<String> {
        self.drive_inner().await
    }

    async fn drive_inner(&mut self) -> Result<String> {
        if self.h1_resolution.is_some() {
            let h1 = self.start_live_h1()?;
            self.ready.push_back(h1);
        } else {
            let sections = self.ctx.prompt().sections.as_slice();
            if sections.is_empty() {
                return Ok(GENERIC_COMPLETION.to_owned());
            }
            self.start_root_walk(sections, &serde_json::json!({}))?;
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
                return Err(Error::Internal(
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
                answer = self.answers.recv() => {
                    let Some((request_id, answer)) = answer else {
                        return Err(Error::Internal(
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
                        return Err(Error::Internal(
                            "an answer arrived for a request with no pending entry and no recorded abort",
                        ));
                    };
                    self.chains[chain_id.index()].incoming = Some(answer);
                    self.ready.push_back(chain_id);
                }
            }
        }
    }

    /// Creates one chain over `slice` from `index` and returns its id. The
    /// chain enters its first section on its first step; `addressed` marks
    /// an addressed arrival (an execute target or a fanout arm's worker),
    /// which runs even when the section is marked off-walk. The chain's
    /// `var` slot seeds from `var` (an execute chain's or arm's caller
    /// snapshot, discarded with the chain). `arm` carries the fanout-arm
    /// state for an arm chain.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the run's chain count exceeds `u32`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the chain keeps its context fork, position, entry mode, parent, var seed, depth, and arm state explicit and linear"
    )]
    fn start_chain(
        &mut self,
        ctx: RunContext,
        slice: &'a [Section],
        index: usize,
        addressed: bool,
        parent: Option<ChainId>,
        var: &serde_json::Value,
        execute_depth: usize,
        arm: Option<ArmState<'a>>,
    ) -> Result<ChainId> {
        if self.chains.len() >= self.max_chains {
            return Err(Error::Internal("a run's chain count cannot exceed u32"));
        }
        let id = ChainId(
            u32::try_from(self.chains.len())
                .map_err(|_| Error::Internal("a run's chain count cannot exceed u32"))?,
        );
        self.chains.push(Chain {
            ctx,
            frame: None,
            slice,
            index,
            positions: Vec::new(),
            addressed,
            block: 0,
            coroutine: None,
            incoming: None,
            reply: None,
            var: var.clone(),
            execute_depth,
            client: None,
            parent,
            arm,
            h1: None,
        });
        Ok(id)
    }

    /// Starts the root walk chain over `sections`, seeded with the H1
    /// pass's hand-off `var` (empty when the drive has no H1 phase), and
    /// enqueues it.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the run's chain count exceeds `u32`.
    fn start_root_walk(&mut self, sections: &'a [Section], var: &serde_json::Value) -> Result<()> {
        let root = self.start_chain(self.ctx.clone(), sections, 0, false, None, var, 0, None)?;
        // Seed the root chain's client slot from the run's configured
        // client, as the legacy walk's slot is seeded from run()'s client:
        // a prose block before any infer must use it rather than fall back
        // to building an environment client.
        self.chains[root.index()].client = self.client.ready().cloned();
        self.ready.push_back(root);
        Ok(())
    }

    /// Starts the live H1 pass as the driver loop's first chain: the
    /// prompt's H1 blocks under its title, driven through the same
    /// coroutine machinery as any section under the live pass's rules.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the run's chain count exceeds `u32`.
    fn start_live_h1(&mut self) -> Result<ChainId> {
        let id = ChainId(
            u32::try_from(self.chains.len())
                .map_err(|_| Error::Internal("a run's chain count cannot exceed u32"))?,
        );
        // The pass owns its client slot, seeded from the run's configured
        // client, exactly as the legacy pass seeds its own.
        let client = self.client.ready().cloned();
        self.chains.push(Chain {
            ctx: self.ctx.clone(),
            frame: None,
            slice: &[],
            index: 0,
            positions: Vec::new(),
            addressed: false,
            block: 0,
            coroutine: None,
            incoming: None,
            reply: None,
            var: serde_json::json!({}),
            execute_depth: 0,
            client,
            parent: None,
            arm: None,
            h1: Some(self.ctx.prompt().h1_blocks.as_slice()),
        });
        Ok(id)
    }

    /// Ends the live H1 pass at its fall-through: the final `var` and reply
    /// read back while the VM is live, then the frame drops unarmed - the
    /// pass never arms completion, so `SECTION_FINISHED` never fires for
    /// it. The root walk then starts from the `var` hand-off under the
    /// walk's own context fork; with no sections the run's result is the
    /// pass's reply, else the shared generic completion.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the final `var` read-back fails,
    /// [`Error::TimestampFormat`] when the walk's `when` fails to format,
    /// or [`Error::Internal`] when the chain holds no frame.
    fn end_live_h1(&mut self, id: ChainId, root_result: &mut Option<Result<String>>) -> Result<()> {
        let chain = &mut self.chains[id.index()];
        let Some(mut frame) = chain.frame.take() else {
            return Err(Error::Internal("the live H1 pass ends with a live frame"));
        };
        let var = frame.read_var()?;
        let reply = frame.reply();
        drop(frame);
        let sections = self.ctx.prompt().sections.as_slice();
        if sections.is_empty() {
            *root_result = Some(Ok(reply.unwrap_or_else(|| GENERIC_COMPLETION.to_owned())));
            return Ok(());
        }
        // The H1-to-walk handoff: the walk's context takes its live `when`;
        // H1's binds already landed in the shared sets the views read.
        let when = now_rfc3339_checked()?;
        let walk_ctx = self.ctx.with_walk_state(&when);
        let root = self.start_chain(walk_ctx, sections, 0, false, None, &var, 0, None)?;
        self.chains[root.index()].client = self.client.ready().cloned();
        self.ready.push_back(root);
        Ok(())
    }

    /// Enters the chain's next section and reports whether one was entered:
    /// skips off-walk sections on fall-through arrival (an addressed
    /// arrival runs its target anyway, and one entry consumes the flag),
    /// then constructs the frame with the next run-global id, seeded from
    /// the chain's reply, `var`, and client slots. `Ok(false)` means the
    /// slice is exhausted and the chain ends.
    ///
    /// # Errors
    /// Returns the [`Error`] of frame construction, as documented on
    /// [`SectionContext::new`].
    fn enter_section(&mut self, id: ChainId) -> Result<bool> {
        let chain = &mut self.chains[id.index()];
        if chain.h1.is_some() {
            // The live H1 pass enters its frame exactly once: id 0 under
            // the prompt's title, the control-stub surface, the shim base,
            // and no SECTION_STARTED - the pass is not a walked section.
            let frame = SectionContext::new_live_h1(&chain.ctx)?;
            chain.frame = Some(frame);
            chain.block = 0;
            return Ok(true);
        }
        // A fanout arm's first entry constructs the worker frame with the
        // arm's own seeds: the collection item, the store-write scope, the
        // caller's cloned `var`, and the worker's visible set for the
        // `list_from_section` callback. Later entries of the arm's walk
        // (after a jump) are plain sections on the walk path below.
        if let Some(arm) = &chain.arm
            && arm.at_worker
        {
            let (worker_slice, worker_index) = (arm.worker_slice, arm.worker_index);
            let (caller_slice, caller_index) = (arm.caller_slice, arm.caller_index);
            let (item_index, item, write_token) =
                (arm.item_index, arm.item.clone(), arm.write_token);
            let worker = &worker_slice[worker_index];
            let caller = &caller_slice[caller_index];
            let home = home_without(&visible_sections(caller_slice, caller), worker);
            let frame = SectionContext::new_fanout_arm(
                &chain.ctx,
                worker,
                &home,
                item_index,
                item,
                write_token,
                chain.reply.as_deref(),
                &chain.var,
            )?;
            chain.frame = Some(frame);
            chain.block = 0;
            return Ok(true);
        }
        let mut index = chain.index;
        while index < chain.slice.len() && !chain.addressed && chain.slice[index].is_off_walk() {
            index += 1;
        }
        chain.index = index;
        chain.addressed = false;
        if index >= chain.slice.len() {
            return Ok(false);
        }
        // `slice` borrows the prompt tree, not the arena, so the frame
        // construction can borrow the chain's own context and slots.
        let slice = chain.slice;
        let frame = SectionContext::new(
            &chain.ctx,
            &slice[index],
            slice,
            next_id(chain.ctx.ids()),
            chain.reply.as_deref(),
            &chain.var,
        )?;
        chain.frame = Some(frame);
        chain.block = 0;
        Ok(true)
    }

    /// Runs one ready chain to its next suspension point. An arm chain's
    /// step runs inside the arm's own cancel scope: the handle is the
    /// run's, cloned at dispatch, so the scope re-installs the same
    /// task-local the driver already runs under - the per-arm wiring the
    /// legacy engine needed a spawn boundary crossing for (PF-CANCEL-002).
    async fn step(&mut self, id: ChainId, root_result: &mut Option<Result<String>>) -> Result<()> {
        let cancel = self.chains[id.index()]
            .arm
            .as_ref()
            .and_then(|arm| arm.cancel.clone());
        // Boxed so the driver future carries a pointer, not the step body.
        cancel::maybe_scope(cancel, Box::pin(self.step_inner(id, root_result))).await
    }

    /// Runs one ready chain to its next suspension point: resume a
    /// suspended coroutine with its delivered answer, or advance the walk -
    /// entering the next section, starting the next Lua block's coroutine,
    /// running one prose block inline, or falling through at a section's
    /// end.
    async fn step_inner(
        &mut self,
        id: ChainId,
        root_result: &mut Option<Result<String>>,
    ) -> Result<()> {
        /// What the chain does next, decided under the chain borrow so the
        /// action phase can touch the scheduler's other fields.
        enum Advance {
            /// Resume the suspended coroutine with its delivered answer.
            Resume(Thread, Answer),
            /// The chain is between sections: enter the next section, or
            /// end the chain when the slice is exhausted.
            EnterSection,
            /// Start the current Lua block as a fresh coroutine.
            StartLua,
            /// Run the current prose block inline.
            RunProse,
            /// The section's blocks are exhausted: fall through.
            SectionEnd,
        }
        let advance = {
            let chain = &mut self.chains[id.index()];
            if let Some(answer) = chain.incoming.take() {
                let Some(thread) = chain.coroutine.take() else {
                    return Err(Error::Internal(
                        "a delivered answer implies a suspended coroutine",
                    ));
                };
                Advance::Resume(thread, answer)
            } else if chain.coroutine.is_some() {
                return Err(Error::Internal(
                    "a ready chain's suspended coroutine waits on its answer",
                ));
            } else if chain.frame.is_none() {
                Advance::EnterSection
            } else if chain.block >= chain.blocks().len() {
                Advance::SectionEnd
            } else {
                match &chain.blocks()[chain.block] {
                    Block::Lua(_) => Advance::StartLua,
                    Block::Prose { .. } => Advance::RunProse,
                }
            }
        };
        match advance {
            Advance::EnterSection => self.advance_entry(id, root_result),
            Advance::Resume(thread, answer) => self.resume_block(id, &thread, answer, root_result),
            Advance::StartLua => self.start_lua(id, root_result),
            Advance::RunProse => self.run_prose(id).await,
            Advance::SectionEnd => {
                if self.chains[id.index()].h1.is_some() {
                    self.end_live_h1(id, root_result)?;
                } else {
                    self.end_section(id)?;
                    self.ready.push_back(id);
                }
                Ok(())
            }
        }
    }

    /// Resumes a chain's suspended coroutine with its delivered answer: the
    /// live H1 pass resumes inside a fresh resolver scope, a walked section
    /// resumes directly.
    fn resume_block(
        &mut self,
        id: ChainId,
        thread: &Thread,
        answer: Answer,
        root_result: &mut Option<Result<String>>,
    ) -> Result<()> {
        let chain = &self.chains[id.index()];
        if chain.h1.is_some() {
            let (result, callback_error) = self.h1_scoped_step(id, |vm, program| {
                vm.resume_block_coro_answer(program, thread, answer)
            })?;
            return self.finish_h1_step(id, result, callback_error, root_result);
        }
        let slice = chain.slice;
        let program = match &slice[chain.index].blocks[chain.block] {
            Block::Lua(program) => program,
            Block::Prose { .. } => {
                return Err(Error::Internal("a suspended coroutine's block is Lua"));
            }
        };
        let frame = chain
            .frame
            .as_ref()
            .ok_or(Error::Internal("a live chain holds its frame"))?;
        let result = frame
            .vm()?
            .resume_block_coro_answer(program, thread, answer);
        self.handle_coro_result(id, result, root_result)
    }

    /// Starts the chain's current Lua block as a fresh coroutine: the live
    /// H1 pass starts it inside a fresh resolver scope, a walked section
    /// starts it directly. The driver owns the chunk observation
    /// boundaries: STARTED at the block's start, SUCCEEDED or FAILED when
    /// its coroutine finally returns or fails - a suspension is neither.
    fn start_lua(&mut self, id: ChainId, root_result: &mut Option<Result<String>>) -> Result<()> {
        let chain = &self.chains[id.index()];
        let observer = Arc::clone(chain.ctx.observer());
        let execution = chain.ctx.execution().to_owned();
        let name = chain.section_name().to_owned();
        observer.observe(&execution, &name, detail::LUA_CHUNK_STARTED);
        if chain.h1.is_some() {
            let (result, callback_error) = self.h1_scoped_step(id, SectionVm::start_block_coro)?;
            return self.finish_h1_step(id, result, callback_error, root_result);
        }
        let slice = chain.slice;
        let program = match &slice[chain.index].blocks[chain.block] {
            Block::Lua(program) => program,
            Block::Prose { .. } => {
                return Err(Error::Internal("the advance matched the block kind"));
            }
        };
        let frame = chain
            .frame
            .as_ref()
            .ok_or(Error::Internal("a live chain holds its frame"))?;
        let result = frame.vm()?.start_block_coro(program);
        self.handle_coro_result(id, result, root_result)
    }

    /// Runs the chain's current prose block inline: the live H1 prose path
    /// for the pass, the shared section prose path on the walk.
    async fn run_prose(&mut self, id: ChainId) -> Result<()> {
        let chain = &mut self.chains[id.index()];
        let (text, loop_capable) = match &chain.blocks()[chain.block] {
            Block::Prose { text, loop_capable } => (text.clone(), *loop_capable),
            Block::Lua(_) => {
                return Err(Error::Internal("the advance matched the block kind"));
            }
        };
        let name = chain.section_name().to_owned();
        let is_h1 = chain.h1.is_some();
        let frame = chain
            .frame
            .as_mut()
            .ok_or(Error::Internal("a live chain holds its frame"))?;
        if is_h1 {
            frame
                .run_live_h1_prose_block(&chain.ctx, &name, &text, loop_capable, &mut chain.client)
                .await?;
        } else {
            frame
                .run_prose_block(&chain.ctx, &name, &text, loop_capable, &mut chain.client)
                .await?;
        }
        chain.block += 1;
        self.ready.push_back(id);
        Ok(())
    }

    /// Enters the chain's next section and requeues it, or finishes the
    /// chain when its slice is exhausted.
    ///
    /// # Errors
    /// Returns the [`Error`] of frame construction, as documented on
    /// [`SectionContext::new`].
    fn advance_entry(
        &mut self,
        id: ChainId,
        root_result: &mut Option<Result<String>>,
    ) -> Result<()> {
        if self.enter_section(id)? {
            self.ready.push_back(id);
        } else if self.pop_position(id) {
            // A jump-started child level exhausted: the parent walk resumes
            // after the jumper with the child walk's last reply (already
            // the chain's reply slot, shared across the descent).
            self.ready.push_back(id);
        } else {
            // The walk ran off the slice's last section: the chain ends,
            // carrying its reply slot.
            self.finish(id, Ok(None), root_result);
        }
        Ok(())
    }

    /// Resumes a jump-suspended parent position when a child level
    /// exhausts, returning `false` when the chain holds no suspended
    /// position - meaning its own root slice exhausted and the chain ends.
    /// The reply and `var` slots need no handling: the child walk shared
    /// them, so they already carry the child level's last values.
    fn pop_position(&mut self, id: ChainId) -> bool {
        let chain = &mut self.chains[id.index()];
        let Some((slice, jumper)) = chain.positions.pop() else {
            return false;
        };
        chain.slice = slice;
        chain.index = jumper + 1;
        chain.addressed = false;
        true
    }

    /// Falls the chain through at its section's end: the reply crosses the
    /// section boundary and the section's final `var` replaces the chain's
    /// clipboard, both read back while the VM is live; the frame's drop is
    /// the teardown boundary, firing `SECTION_FINISHED` for this completed
    /// section; then the walk advances to the next section.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the final `var` read-back fails (the
    /// frame drops unarmed, as on the legacy path), or
    /// [`Error::Internal`] when the chain holds no frame.
    fn end_section(&mut self, id: ChainId) -> Result<()> {
        let chain = &mut self.chains[id.index()];
        let Some(mut frame) = chain.frame.take() else {
            return Err(Error::Internal("a section end implies a live frame"));
        };
        chain.reply = frame.reply();
        chain.var = frame.read_var()?;
        frame.mark_completed();
        drop(frame);
        if let Some(arm) = &mut chain.arm {
            // The worker's own entry is complete; the arm's walk continues
            // (or ends) as plain sections, exactly as after a jump out.
            arm.at_worker = false;
        }
        chain.index += 1;
        Ok(())
    }

    /// Applies a jump's control transfer: closes the jumper's frame as
    /// completed (the reply read back from the jumper's VM, so an author's
    /// `reply = nil` or custom string steers the target; the final `var`
    /// rolled forward; the armed drop firing `SECTION_FINISHED`, a jump
    /// being a completion), resolves the heading against the jumper's
    /// visible set, and moves the walk. A sibling target sets the index
    /// within the target's slice, addressed; a child target pushes the
    /// current position onto the chain's position stack and descends into
    /// the jumper's child slice from the target, addressed.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the reply or `var` read-back fails (the
    /// frame drops unarmed, as on the legacy path) or when the heading
    /// matches no visible section or more than one - the jumper's frame has
    /// already closed as completed, exactly as the legacy walk resolves
    /// after the jumper's teardown.
    fn apply_jump(&mut self, id: ChainId, heading: &str) -> Result<()> {
        let (slice, index) = {
            let chain = &mut self.chains[id.index()];
            let Some(mut frame) = chain.frame.take() else {
                return Err(Error::Internal("a jump implies a live frame"));
            };
            frame.read_reply()?;
            chain.reply = frame.reply();
            chain.var = frame.read_var()?;
            frame.mark_completed();
            drop(frame);
            (chain.slice, chain.index)
        };
        let target = self.resolve_chain_target(id, heading)?;
        let chain = &mut self.chains[id.index()];
        if let Some(arm) = &mut chain.arm {
            // The worker's own entry is left behind by the transfer; later
            // entries of the arm's walk are plain sections.
            arm.at_worker = false;
        }
        if target.child {
            chain.positions.push((slice, index));
        }
        chain.slice = target.slice;
        chain.index = target.index;
        chain.addressed = true;
        Ok(())
    }

    /// Resolves `heading` against the chain's current section's visible set
    /// and returns the slice the walk or a contained chain continues on:
    /// the jumper's child slice for a direct child, the target's own slice
    /// otherwise.
    ///
    /// For an arm chain still at its worker, the visible set is the fanout
    /// caller's visible set minus the worker, plus the worker's children -
    /// the set the legacy arm's control globals resolve over.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the heading is malformed, matches no
    /// visible section, or matches more than one (see
    /// [`fanout::resolve_sibling`]).
    fn resolve_chain_target(&self, id: ChainId, heading: &str) -> Result<ChainTarget<'a>> {
        let chain = &self.chains[id.index()];
        if let Some(arm) = &chain.arm
            && arm.at_worker
        {
            let (caller_slice, caller_index) = (arm.caller_slice, arm.caller_index);
            let (worker_slice, worker_index) = (arm.worker_slice, arm.worker_index);
            return resolve_arm_target(
                caller_slice,
                caller_index,
                worker_slice,
                worker_index,
                heading,
            );
        }
        let slice = chain.slice;
        let index = chain.index;
        // `slice` borrows the prompt tree, not the arena, so the jumper
        // outlives the chain borrow above.
        let jumper = &slice[index];
        match resolve_jump_target(heading, slice, jumper)? {
            JumpTarget::Child(child) => Ok(ChainTarget {
                slice: &jumper.children,
                index: child,
                child: true,
            }),
            JumpTarget::Sibling(sibling) => Ok(ChainTarget {
                slice,
                index: sibling,
                child: false,
            }),
        }
    }

    /// Applies one Lua block coroutine's outcome: parks a yielded chain on
    /// its request's dispatch, advances or finishes a completed block, and
    /// reports the chunk's closing observation boundary.
    fn handle_coro_result(
        &mut self,
        id: ChainId,
        result: Result<CoroStep>,
        root_result: &mut Option<Result<String>>,
    ) -> Result<()> {
        let (observer, execution, name) = {
            let chain = &self.chains[id.index()];
            (
                Arc::clone(chain.ctx.observer()),
                chain.ctx.execution().to_owned(),
                chain.section_name().to_owned(),
            )
        };
        let step = match result {
            Ok(step) => step,
            Err(error) => {
                observer.observe(&execution, &name, detail::LUA_CHUNK_FAILED);
                return Err(error);
            }
        };
        match step {
            CoroStep::Yielded(thread, values) => {
                let chain = &mut self.chains[id.index()];
                let frame = chain
                    .frame
                    .as_ref()
                    .ok_or(Error::Internal("a live chain holds its frame"))?;
                match frame.vm()?.request_from_yield(&values) {
                    YieldParse::Request(request) => {
                        chain.coroutine = Some(thread);
                        self.dispatch(id, request)
                    }
                    YieldParse::Call(answer) => {
                        // An argument-validation failure is the call's
                        // answer: the shim raises it at the call site, so
                        // an author `pcall` catches it exactly as on the
                        // legacy callback path.
                        chain.coroutine = Some(thread);
                        chain.incoming = Some(answer);
                        self.ready.push_back(id);
                        Ok(())
                    }
                    YieldParse::Malformed(error) => {
                        observer.observe(&execution, &name, detail::LUA_CHUNK_FAILED);
                        Err(error)
                    }
                }
            }
            CoroStep::Done(LuaBlockResult::Jump(heading)) => {
                // A jump is a control transfer, not a failure: the chunk
                // boundary reports success and the walk moves to the
                // resolved target.
                observer.observe(&execution, &name, detail::LUA_CHUNK_SUCCEEDED);
                if self.chains[id.index()].h1.is_some() {
                    // The H1 VM carries only the stub control globals, which
                    // raise before anything is recorded; this arm stays
                    // defensive against a recorded jump, exactly as the
                    // legacy `run_live_h1_block` maps it.
                    return Err(Error::Lua(format!(
                        "jump({heading}) is not available in live H1 Lua"
                    )));
                }
                self.apply_jump(id, &heading)?;
                self.ready.push_back(id);
                Ok(())
            }
            CoroStep::Done(LuaBlockResult::Returned(value)) => {
                observer.observe(&execution, &name, detail::LUA_CHUNK_SUCCEEDED);
                if self.chains[id.index()].h1.is_some() {
                    let chain = &mut self.chains[id.index()];
                    if let Some(value) = value {
                        // A scalar return from the live H1 pass
                        // short-circuits the whole run. The final `var`
                        // read-back runs here exactly as the legacy pass
                        // reads it on every exit, so a reassigned `var`
                        // global fails the run instead of returning the
                        // value; the frame then drops unarmed - the pass
                        // never fires SECTION_FINISHED.
                        let mut frame = chain
                            .frame
                            .take()
                            .ok_or(Error::Internal("a live chain holds its frame"))?;
                        frame.read_var()?;
                        drop(frame);
                        *root_result = Some(Ok(value));
                        return Ok(());
                    }
                    // Live H1 does not read the `reply` global back after a
                    // Lua block: the pass's reply slot rolls forward through
                    // prose alone.
                    chain.block += 1;
                    self.ready.push_back(id);
                    return Ok(());
                }
                if let Some(value) = value {
                    // A scalar return ends the chain it fired in.
                    self.finish(id, Ok(Some(value)), root_result);
                    return Ok(());
                }
                // The `reply` global is the author-writable shadow of the
                // walk's reply: read it back after each chunk so an
                // author's `reply = nil` (or a custom string) steers the
                // next prose and the chain's finish.
                let chain = &mut self.chains[id.index()];
                let frame = chain
                    .frame
                    .as_mut()
                    .ok_or(Error::Internal("a live chain holds its frame"))?;
                frame.read_reply()?;
                chain.block += 1;
                self.ready.push_back(id);
                Ok(())
            }
        }
    }

    /// Runs one live H1 coroutine step (a block's start or a suspension's
    /// resume) inside a fresh Lua scope with the capability resolvers and
    /// the live models shim wrap installed. The step's outcome and the
    /// captured resolver callback error return separately, so the caller
    /// observes the chunk's own boundary first and then applies the legacy
    /// `run_live_h1_block` contract: a typed resolver error captured by a
    /// callback fails the block even when the chunk caught the Lua error
    /// itself.
    ///
    /// The resolvers reinstall on every step because their callbacks are
    /// scoped: a suspended coroutine outlives the scope it started in, so
    /// each resume enters a fresh scope with fresh live tables before the
    /// thread runs again. The resolution's decision cache is run-scoped, so
    /// reinstalling never re-queries the picker. One legacy edge narrows
    /// here: an author alias saved from the `tools`/`models` table (say
    /// `local bind = models.bind`) dies with its scope, so calling it after
    /// a suspension fails where the legacy block-scoped install allowed it
    /// within one block.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the chain is not the live H1 chain
    /// or the step machinery fails; the step's own outcome and the
    /// captured resolver callback error ride the `Ok` pair.
    fn h1_scoped_step(
        &self,
        id: ChainId,
        run: impl FnOnce(&SectionVm, &LuaProgram) -> Result<CoroStep>,
    ) -> Result<(Result<CoroStep>, Option<Error>)> {
        let chain = &self.chains[id.index()];
        let Some(blocks) = chain.h1 else {
            return Err(Error::Internal(
                "the scoped step belongs to the live H1 pass",
            ));
        };
        let program = match &blocks[chain.block] {
            Block::Lua(program) => program,
            Block::Prose { .. } => {
                return Err(Error::Internal("a suspended coroutine's block is Lua"));
            }
        };
        let resolution = self
            .h1_resolution
            .as_ref()
            .ok_or(Error::Internal("the live H1 pass holds its resolution"))?;
        let frame = chain
            .frame
            .as_ref()
            .ok_or(Error::Internal("a live chain holds its frame"))?;
        let vm = frame.vm()?;
        let mut outcome = None;
        let scoped = vm.lua().scope(|scope| {
            resolution
                .install(vm.lua(), scope)
                .map_err(mlua::Error::external)?;
            shim_live_h1_models(vm.lua()).map_err(mlua::Error::external)?;
            outcome = Some(run(vm, program));
            Ok(())
        });
        let result = match scoped {
            Ok(()) => outcome.ok_or(Error::Internal("the scoped step records its outcome"))?,
            Err(error) => Err(Error::lua(error)),
        };
        // The outcome and the captured callback error travel separately:
        // the outcome decides the chunk's observation boundary, and the
        // callback error is reported after it, with precedence.
        Ok((result, resolution.take_callback_error()?))
    }

    /// Applies one live H1 step's outcome, then its captured resolver
    /// callback error: the outcome drives the chunk's observation boundary
    /// (a chunk that caught the resolver's Lua error itself still reports
    /// `LUA_CHUNK_SUCCEEDED`), and the callback error fails the run
    /// afterward, taking precedence over the outcome - the legacy
    /// `run_live_h1_block` mapping, where the callback check follows the
    /// chunk's own boundary.
    fn finish_h1_step(
        &mut self,
        id: ChainId,
        result: Result<CoroStep>,
        callback_error: Option<Error>,
        root_result: &mut Option<Result<String>>,
    ) -> Result<()> {
        let outcome = self.handle_coro_result(id, result, root_result);
        match callback_error {
            Some(error) => Err(error),
            None => outcome,
        }
    }

    /// Dispatches one validated request from a suspended chain.
    ///
    /// # Errors
    /// Returns the typed protocol error for a received `mcp` request, which
    /// no call surface produces yet.
    fn dispatch(&mut self, id: ChainId, request: Request) -> Result<()> {
        match request {
            Request::Infer { prompt, binding } => {
                self.dispatch_infer(id, prompt, binding);
                Ok(())
            }
            Request::Execute { target, input, var } => {
                self.dispatch_execute(id, &target, input.as_deref(), &var);
                Ok(())
            }
            Request::Fanout { worker, items, var } => {
                self.dispatch_fanout(id, &worker, &items, &var);
                Ok(())
            }
            Request::Mcp { .. } => Err(Request::mcp_reserved()),
        }
    }

    /// Dispatches an `infer` request: resolves the binding and the chain's
    /// client, spawns the single gateway round onto the answer channel, and
    /// parks the chain in the pending table. A resolution failure is the
    /// call's answer, resumed into the caller so an author `pcall` can catch
    /// it exactly as on the legacy callback path.
    fn dispatch_infer(&mut self, id: ChainId, prompt: String, binding: Option<ModelBinding>) {
        match self.prepare_infer(id, prompt, binding) {
            Ok((request_id, task)) => {
                self.io_tasks.insert(request_id, task.abort_handle());
                self.pending.insert(request_id, id);
            }
            Err(error) => {
                self.chains[id.index()].incoming = Some(Answer::Infer(Err(error)));
                self.ready.push_back(id);
            }
        }
    }

    /// The fallible half of infer dispatch: the binding resolution (the
    /// handle's frozen binding, else the section's current model), the lazy
    /// client resolution, and the spawned round.
    fn prepare_infer(
        &mut self,
        id: ChainId,
        prompt: String,
        binding: Option<ModelBinding>,
    ) -> Result<(RequestId, tokio::task::JoinHandle<()>)> {
        let chain = &mut self.chains[id.index()];
        let binding = if let Some(binding) = binding {
            binding
        } else {
            let frame = chain
                .frame
                .as_ref()
                .ok_or(Error::Internal("a live chain holds its frame"))?;
            resolve_model_binding(chain.ctx.models(), &frame.vm()?.model_runtime)?.ok_or_else(
                || Error::ModelRequired {
                    section: chain.section_name().to_owned(),
                },
            )?
        };
        if chain.client.is_none() {
            chain.client = Some(self.client.resolve()?);
        }
        let client = chain
            .client
            .as_ref()
            .ok_or(Error::Internal("the client slot was just resolved"))?
            .clone();
        let observer = Arc::clone(chain.ctx.observer());
        let debug = chain.ctx.debug().cloned();
        let execution = chain.ctx.execution().to_owned();
        let section = chain.section_name().to_owned();
        let turns = Arc::clone(chain.ctx.turns());
        let request_id = RequestId(self.next_request);
        self.next_request += 1;
        let tx = self.answer_tx.clone();
        let task = tokio::spawn(async move {
            let result = infer_round(
                &client,
                &binding,
                &prompt,
                observer.as_ref(),
                debug.as_deref(),
                &execution,
                &section,
                &turns,
            )
            .await;
            // A send fails only when the driver is gone (a cancelled run);
            // the answer is then moot.
            let _ = tx.send((request_id, Answer::Infer(result)));
        });
        Ok((request_id, task))
    }

    /// Dispatches an `execute` request: constructs the child chain, pushes
    /// it on the chain stack, and enqueues it; the parent blocks until the
    /// child's finish delivers its final text as the answer. Every dispatch
    /// failure - the depth cap, target resolution, child construction - is
    /// the call's answer, resumed into the caller so an author `pcall` can
    /// catch it exactly as on the legacy callback path.
    fn dispatch_execute(
        &mut self,
        id: ChainId,
        target: &str,
        input: Option<&str>,
        var: &serde_json::Value,
    ) {
        match self.prepare_execute(id, target, input, var) {
            Ok(child) => {
                self.stack.push(child);
                self.ready.push_back(child);
            }
            Err(error) => {
                self.chains[id.index()].incoming = Some(Answer::Execute(Err(error)));
                self.ready.push_back(id);
            }
        }
    }

    /// The fallible half of execute dispatch: the depth cap checked against
    /// the caller's execute-depth field, the target resolved over the
    /// caller's visible set, and the child chain constructed one level
    /// deeper under the call's args and `var` snapshot.
    fn prepare_execute(
        &mut self,
        id: ChainId,
        target: &str,
        input: Option<&str>,
        var: &serde_json::Value,
    ) -> Result<ChainId> {
        let chain = &self.chains[id.index()];
        if chain.h1.is_some() {
            // Unreachable: the H1 control stubs raise before anything can
            // yield. A panic on the empty walk slice would be worse than
            // the typed invariant error.
            return Err(Error::Internal(
                "the live H1 pass cannot dispatch an execute request",
            ));
        }
        let depth = chain.execute_depth + 1;
        if depth > MAX_EXECUTE_DEPTH {
            return Err(Error::Lua(format!(
                "execute recursion exceeded cap of {MAX_EXECUTE_DEPTH}"
            )));
        }
        let args = input.unwrap_or_else(|| chain.ctx.args()).to_owned();
        let child_ctx = chain.ctx.with_args(&args);
        let client = chain.client.clone();
        // `chain`'s arena borrow ends here; the resolution borrows the
        // prompt tree, so the target's slice outlives it.
        let target_section = self.resolve_chain_target(id, target)?;
        let child = self.start_chain(
            child_ctx,
            target_section.slice,
            target_section.index,
            true,
            Some(id),
            var,
            depth,
            None,
        )?;
        // The child inherits the caller's client slot: an already-resolved
        // client is shared, an unresolved one stays lazy.
        self.chains[child.index()].client = client;
        Ok(child)
    }

    /// Dispatches a `fanout` request: resolves the worker, creates the join
    /// state with its preallocated per-index result slots, and starts the
    /// first window of arm chains; the parent blocks until the join
    /// completes. Every dispatch failure - the depth cap, an empty
    /// collection, worker resolution - is the call's answer, resumed into
    /// the caller so an author `pcall` can catch it exactly as on the
    /// legacy callback path.
    fn dispatch_fanout(
        &mut self,
        id: ChainId,
        worker: &str,
        items: &[serde_json::Value],
        var: &serde_json::Value,
    ) {
        match self.prepare_fanout(id, worker, items, var) {
            Ok(()) => {}
            Err(error) => {
                self.chains[id.index()].incoming = Some(Answer::Fanout(Err(error)));
                self.ready.push_back(id);
            }
        }
    }

    /// The fallible half of fanout dispatch: the depth cap checked against
    /// the caller's execute-depth field (each arm runs one level deeper),
    /// the empty collection rejected before any scheduling, the worker
    /// resolved over the caller's visible set, and the join state and
    /// first window of arm chains created.
    fn prepare_fanout(
        &mut self,
        id: ChainId,
        worker_name: &str,
        items: &[serde_json::Value],
        var: &serde_json::Value,
    ) -> Result<()> {
        let chain = &self.chains[id.index()];
        if chain.h1.is_some() {
            // Unreachable: the H1 control stubs raise before anything can
            // yield. A panic on the empty walk slice would be worse than
            // the typed invariant error.
            return Err(Error::Internal(
                "the live H1 pass cannot dispatch a fanout request",
            ));
        }
        let depth = chain.execute_depth + 1;
        if depth > MAX_EXECUTE_DEPTH {
            return Err(Error::Lua(format!(
                "fanout recursion exceeded cap of {MAX_EXECUTE_DEPTH}"
            )));
        }
        // An empty collection runs zero arms; that is an authoring bug (a
        // list section that parsed empty, a wrong variable), not a valid
        // run.
        if items.is_empty() {
            return Err(Error::Lua(
                "fanout over an empty collection: no work is likely a bug".to_owned(),
            ));
        }
        // An at-worker arm's fanout resolves over the worker's visible set
        // (handled inside `resolve_chain_target`); the new arms in turn
        // treat the worker as their caller.
        let (caller_slice, caller_index) = match &chain.arm {
            Some(arm) if arm.at_worker => (arm.worker_slice, arm.worker_index),
            _ => (chain.slice, chain.index),
        };
        let ctx = chain.ctx.clone();
        let reply = chain.reply.clone();
        let client = chain.client.clone();
        // `chain`'s arena borrow ends here; the resolution borrows the
        // prompt tree, so the worker's slice outlives it.
        let target = self.resolve_chain_target(id, worker_name)?;
        let worker = &target.slice[target.index];
        if worker.prologue().is_none() && worker.epilog().is_none() && !worker.items.is_empty() {
            return Err(Error::Lua(format!(
                "section `{}` is a list section, not a worker template",
                worker.name
            )));
        }
        let fanout_id = FanoutId(self.next_fanout);
        self.next_fanout += 1;
        self.joins.insert(
            fanout_id,
            JoinState {
                remaining: items.len(),
                results: vec![None; items.len()],
                parent: id,
                active: 0,
                next: 0,
                items: items.to_vec(),
                window: ctx.limits().fanout_concurrency().get(),
                template: ArmTemplate {
                    caller_slice,
                    caller_index,
                    worker_slice: target.slice,
                    worker_index: target.index,
                    // Arms report through the run's own observer and debug
                    // sink directly - the legacy proxies exist to cross the
                    // spawned-task boundary, which a chain never crosses -
                    // while the fanout's turn counter stays fresh, so arm
                    // turns count against the fanout's own cap.
                    ctx: ctx.with_effective_handles(
                        Arc::clone(ctx.observer()),
                        ctx.debug().cloned(),
                        Arc::new(AtomicU32::new(0)),
                    ),
                    write_token: ctx.store().next_write_token(),
                    reply,
                    var: var.clone(),
                    execute_depth: depth,
                    client,
                    cancel: cancel::current(),
                },
            },
        );
        // A mid-refill failure (the run's chain count exceeding the bound)
        // must not propagate with the join live and a partial window
        // enqueued: the caller resumes with this error as the fanout's
        // answer, and a late arm completion against the live join would
        // resume the parent a second time. Tear the fanout down instead -
        // the join goes and the started arms abort, each finalizer drop
        // reporting FANOUT_ARM_CANCELLED exactly as on fail_fanout's path -
        // and leave the parent's answer to the caller.
        if let Err(error) = self.refill_fanout(fanout_id) {
            self.joins.remove(&fanout_id);
            for arm in self.arm_chains_of(fanout_id) {
                self.abort_subtree(arm);
            }
            return Err(error);
        }
        Ok(())
    }

    /// Starts arm chains for one fanout while a window slot is free and
    /// items remain, enqueuing each on the ready queue. Each arm is a chain
    /// over the worker alone (a singleton slice), addressed so an off-walk
    /// worker runs; a jump out of the worker retargets the arm's walk.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the join is not live or the run's
    /// chain count exceeds `u32`.
    fn refill_fanout(&mut self, fanout: FanoutId) -> Result<()> {
        loop {
            let (index, item, template) = {
                let Some(join) = self.joins.get_mut(&fanout) else {
                    return Err(Error::Internal("a window refill implies a live join"));
                };
                if join.next >= join.items.len() || join.active >= join.window {
                    return Ok(());
                }
                let index = join.next;
                join.next += 1;
                join.active += 1;
                (index, join.items[index].clone(), join.template.clone())
            };
            let worker_slice = template.worker_slice;
            let worker = &worker_slice[template.worker_index];
            // Arm creation is the dispatch boundary, so it carries the
            // arm's STARTED observation, exactly as the legacy arm task's
            // start did; the finalizer guards the exactly-once terminal
            // event from here on.
            template.ctx.observer().observe(
                template.ctx.execution(),
                &worker.name,
                detail::FANOUT_ARM_STARTED,
            );
            let arm = ArmState {
                fanout,
                item_index: index,
                item,
                write_token: template.write_token,
                at_worker: true,
                caller_slice: template.caller_slice,
                caller_index: template.caller_index,
                worker_slice,
                worker_index: template.worker_index,
                cancel: template.cancel.clone(),
                finalizer: ArmFinalizer::new(
                    Arc::clone(template.ctx.observer()),
                    template.ctx.execution().to_owned(),
                    worker.name.clone(),
                ),
            };
            let chain = self.start_chain(
                template.ctx.clone(),
                std::slice::from_ref(worker),
                0,
                true,
                None,
                &template.var,
                template.execute_depth,
                Some(arm),
            )?;
            // The arm inherits the caller's reply seed and client slot, as
            // the legacy arm's payload carries them.
            self.chains[chain.index()].reply.clone_from(&template.reply);
            self.chains[chain.index()]
                .client
                .clone_from(&template.client);
            self.ready.push_back(chain);
        }
    }

    /// Applies one arm chain's end to its join, finishing the arm's
    /// terminal observation with its real outcome: a success writes the
    /// arm's preallocated slot (so results land in collection order) and
    /// refills the window; the last arm's landing resumes the parent with
    /// the packed sequence. [`Error::ToolLoopExhausted`] soft-degrades the
    /// arm to the incomplete stub, so one stuck arm cannot kill sibling
    /// evidence. Any other arm error is fatal: it fails the join and
    /// aborts the sibling arms.
    fn complete_arm(&mut self, mut arm: ArmState<'a>, outcome: Result<String>) {
        /// How the join moves on one arm's end.
        enum ArmEnd {
            /// The slot is written and arms remain: refill the window.
            Continue,
            /// The last arm landed: pack the sequence for the parent.
            Complete,
            /// A fatal arm error: fail the fanout and abort the siblings.
            Fail(Error),
        }
        let end = {
            let Some(join) = self.joins.get_mut(&arm.fanout) else {
                // The join already failed on a sibling's fatal error and was
                // removed; this arm's outcome is discarded with it, and the
                // arm's drop reports the cancelled terminal event.
                return;
            };
            join.active -= 1;
            match outcome {
                Ok(text) => {
                    arm.finalizer.finish(detail::FANOUT_ARM_SUCCEEDED);
                    join.results[arm.item_index] = Some(LuaFanoutResult::success(arm.item, text));
                    join.remaining -= 1;
                    if join.remaining == 0 {
                        ArmEnd::Complete
                    } else {
                        ArmEnd::Continue
                    }
                }
                // One stuck arm must not kill sibling evidence facets.
                Err(Error::ToolLoopExhausted) => {
                    let stub = format!(
                        "## {}\n\nUNKNOWN\n\n(section incomplete: tool loop exhausted)",
                        subst::render_item(&arm.item)
                    );
                    arm.finalizer.finish(detail::FANOUT_ARM_EXHAUSTED);
                    join.results[arm.item_index] =
                        Some(LuaFanoutResult::exhausted_stub(arm.item, stub));
                    join.remaining -= 1;
                    if join.remaining == 0 {
                        ArmEnd::Complete
                    } else {
                        ArmEnd::Continue
                    }
                }
                Err(error) => {
                    arm.finalizer.finish(detail::FANOUT_ARM_FAILED);
                    ArmEnd::Fail(error)
                }
            }
        };
        match end {
            ArmEnd::Continue => {
                if let Err(error) = self.refill_fanout(arm.fanout) {
                    self.fail_fanout(arm.fanout, error);
                }
            }
            ArmEnd::Complete => {
                let Some(join) = self.joins.remove(&arm.fanout) else {
                    return;
                };
                // Every slot is Some here: `remaining` reached zero, so
                // every arm wrote its slot. The `ok_or_else` keeps that
                // invariant guarded, mirroring the legacy driver's check.
                let results = join
                    .results
                    .into_iter()
                    .enumerate()
                    .map(|(index, slot)| {
                        slot.ok_or_else(|| {
                            Error::Lua(format!("fanout arm {} finished without a reply", index + 1))
                        })
                    })
                    .collect();
                self.chains[join.parent.index()].incoming = Some(Answer::Fanout(results));
                self.ready.push_back(join.parent);
            }
            ArmEnd::Fail(error) => self.fail_fanout(arm.fanout, error),
        }
    }

    /// Fails one fanout's join: the sibling arms still alive are aborted
    /// (the legacy `JoinSet::abort_all` port - an aborted arm's frame drops
    /// unarmed and its finalizer reports `FANOUT_ARM_CANCELLED`), the
    /// parent resumes with the error, and the join is removed. Items never
    /// dispatched stay unstarted: with the join gone, no refill can create
    /// their arms.
    fn fail_fanout(&mut self, fanout: FanoutId, error: Error) {
        let Some(join) = self.joins.remove(&fanout) else {
            return;
        };
        for sibling in self.arm_chains_of(fanout) {
            self.abort_subtree(sibling);
        }
        self.chains[join.parent.index()].incoming = Some(Answer::Fanout(Err(error)));
        self.ready.push_back(join.parent);
    }

    /// The arena ids of one fanout's live arm chains. An arm whose chain
    /// already finished is absent: `finish` took its arm state, so only
    /// arms still running, suspended, or blocked carry it.
    fn arm_chains_of(&self, fanout: FanoutId) -> Vec<ChainId> {
        // The arena is u32-bounded at insertion (`start_chain`), so the
        // index conversion cannot fail.
        self.chains
            .iter()
            .enumerate()
            .filter(|(_, chain)| chain.arm.as_ref().is_some_and(|arm| arm.fanout == fanout))
            .filter_map(|(index, _)| u32::try_from(index).ok().map(ChainId))
            .collect()
    }

    /// Aborts one chain and everything it transitively blocks on - its
    /// execute children and the arms of its nested fanouts - the scheduler
    /// port of dropping a spawned arm task: the chain leaves the ready
    /// queue and the pending table, its in-flight leaf I/O task is aborted,
    /// and its state drops in the teardown order (the suspended coroutine,
    /// then the frame unarmed - no `SECTION_FINISHED` - then the arm state,
    /// whose finalizer drop reports `FANOUT_ARM_CANCELLED`).
    fn abort_subtree(&mut self, id: ChainId) {
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
            if let Some(task) = self.io_tasks.remove(&request) {
                task.abort();
            }
        }
        // A chain on the execute stack is the top here: only its own
        // descendants sit above it, and the recursion already removed them.
        if self.stack.last() == Some(&id) {
            self.stack.pop();
        }
        let chain = &mut self.chains[id.index()];
        chain.coroutine = None;
        chain.incoming = None;
        chain.frame = None;
        chain.arm = None;
    }

    /// Finishes one chain: the frame's teardown boundary when the chain
    /// ends mid-section, then the outcome's delivery - the run's result for
    /// the root chain, the execute answer for a child chain, the join
    /// slot's result for a fanout arm.
    ///
    /// `outcome` is the chain's end: a scalar return's value, `None` for a
    /// walk that ran off its slice's last section (the chain's reply slot
    /// is the text), or the chain's failure.
    fn finish(
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
        let reply = chain.reply.clone();
        // The live H1 pass never arms completion: SECTION_FINISHED is a
        // walked section's boundary, not the setup pass's. Its completion
        // paths (fall-through, scalar return) handle the frame themselves;
        // this guard keeps an H1 frame that reaches here - an error path -
        // unarmed.
        let is_h1 = chain.h1.is_some();
        let outcome = outcome.and_then(|returned| {
            // A chain ending mid-section (a scalar return) reads its final
            // var back before teardown, exactly as a completed section does
            // at fall-through (the walk rolls it forward; an execute chain
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
                None => match reply {
                    Some(reply) => reply,
                    // The legacy mapping: the top-level chain falls back to
                    // the shared generic completion; an execute chain or a
                    // fanout arm to the empty string.
                    None if parent.is_none() && arm.is_none() => GENERIC_COMPLETION.to_owned(),
                    None => String::new(),
                },
            };
            Ok(text)
        });
        // The frame drops here: the single teardown boundary.
        drop(frame);
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
                    "a finishing child chain is the execute stack's top"
                );
                self.chains[parent_id.index()].incoming = Some(Answer::Execute(outcome));
                self.ready.push_back(parent_id);
            }
        }
    }
}
