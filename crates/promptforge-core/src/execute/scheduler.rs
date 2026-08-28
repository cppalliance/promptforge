//! The chain-stack scheduler: the coroutine protocol's driver loop.
//!
//! One [`Scheduler`] per run, created and owned by the top-level run call
//! and living entirely in the driver loop's stack frame: no `Arc`, no
//! `Mutex`, no sharing. One thread runs everything, and the Lua shims only
//! yield (they never call into Rust for suspending operations), so the
//! scheduler state is unreachable from Lua. The driver body runs inside a
//! [`tokio::task::LocalSet`] so leaf dispatch can `spawn_local`; on a
//! current-thread runtime this is the whole story, and on a multi-thread
//! runtime the run simply never leaves its one thread.
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
//! This module carries the scheduler core plus the core walk rules:
//! sections run in fall-through order, a section marked off-walk is skipped
//! unless the arrival is addressed (an execute target runs anyway), and the
//! reply and `var` roll forward across sections while every section entry
//! takes the next run-global id. Jumps and the fanout dispatch land in
//! later steps; a received `fanout` or `mcp` request is the protocol's
//! typed reserved error.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use mlua::Thread;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::client::GatewayClient;
use crate::lua::{CoroStep, LuaBlockResult, LuaFanoutResult, resolve_model_binding};
use crate::model::ModelBinding;
use crate::observe::detail;
use crate::parser::{Block, Section};
use crate::{Error, Result, cancel};

use super::context::RunContext;
use super::engine::{JumpTarget, resolve_jump_target};
use super::gateway::GatewaySource;
use super::protocol::{Answer, Request};
use super::section_context::SectionContext;
use super::section_vm::VmSetupMode;
use super::support::{GENERIC_COMPLETION, MAX_EXECUTE_DEPTH, next_id};
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
/// per-index result slots (collection order, never finish order), and the
/// parent chain blocked on the join.
///
/// Defined with the scheduler's structures; exercised by the fanout step.
#[allow(dead_code)]
#[derive(Debug)]
struct JoinState {
    /// Arms still running; at zero the parent resumes with the sequence.
    remaining: usize,
    /// One slot per collection index, so results land in collection order.
    results: Vec<Option<LuaFanoutResult>>,
    /// The chain that yielded the fanout, blocked until the join completes.
    parent: ChainId,
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
    /// which outlives the scheduler.
    slice: &'a [Section],
    /// The section of `slice` the chain is running, or the next entry
    /// candidate while the chain is between sections.
    index: usize,
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
    ///
    /// Defined with the scheduler's structures; exercised by the fanout step.
    #[allow(dead_code)]
    joins: HashMap<FanoutId, JoinState>,
    /// The send half every spawned leaf task posts its answer to. The
    /// channel is unbounded: each task sends exactly once, and the in-flight
    /// count is already bounded by the chains that produced them.
    answer_tx: mpsc::UnboundedSender<(RequestId, Answer)>,
    /// The receive half the driver awaits when no chain is ready.
    answers: mpsc::UnboundedReceiver<(RequestId, Answer)>,
    /// Abort handles of the in-flight leaf I/O tasks, aborted on
    /// cancellation; aborting a completed task is a no-op.
    io_tasks: Vec<AbortHandle>,
    /// The next leaf-request id.
    next_request: u64,
    /// The next fanout id.
    ///
    /// Defined with the scheduler's structures; exercised by the fanout step.
    #[allow(dead_code)]
    next_fanout: u32,
    /// The run's gateway source: chains resolve their client slot through
    /// it on first inference.
    client: GatewaySource,
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
            io_tasks: Vec::new(),
            next_request: 0,
            next_fanout: 0,
            client: GatewaySource::from_optional(client, ctx.limits()),
        }
    }

    /// Drives the root chain over the prompt's sections until it ends and
    /// returns the run's result.
    ///
    /// The driver body runs inside a [`tokio::task::LocalSet`] so leaf
    /// dispatch can `spawn_local`.
    ///
    /// # Errors
    /// Returns the [`Error`] of whichever step failed: frame construction,
    /// a Lua block, prose inference, or a dispatched request's answer.
    /// Returns [`Error::Interrupted`] when the run's cancellation handle is
    /// signaled while chains are running or suspended.
    pub(crate) async fn drive(&mut self) -> Result<String> {
        tokio::task::LocalSet::new()
            .run_until(self.drive_inner())
            .await
    }

    async fn drive_inner(&mut self) -> Result<String> {
        let sections = self.ctx.prompt().sections.as_slice();
        if sections.is_empty() {
            return Ok(GENERIC_COMPLETION.to_owned());
        }
        let root = self.start_chain(
            self.ctx.clone(),
            sections,
            0,
            false,
            None,
            &serde_json::json!({}),
            0,
        )?;
        // Seed the root chain's client slot from the run's configured
        // client, as the legacy walk's slot is seeded from run()'s client:
        // a prose block before any infer must use it rather than fall back
        // to building an environment client.
        self.chains[root.index()].client = self.client.ready().cloned();
        self.ready.push_back(root);
        let mut root_result = None;
        loop {
            while let Some(id) = self.ready.pop_front() {
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
                // hook-driven path while running.
                () = cancel::wait_cancelled() => {
                    for handle in &self.io_tasks {
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
                    let Some(chain_id) = self.pending.remove(&request_id) else {
                        return Err(Error::Internal(
                            "an answer arrived for a request no chain is pending on",
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
    /// an addressed arrival (an execute target), which runs even when the
    /// section is marked off-walk. The chain's `var` slot seeds from `var`
    /// (an execute chain's caller snapshot, discarded with the chain).
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the run's chain count exceeds `u32`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the chain keeps its context fork, position, entry mode, parent, var seed, and depth explicit and linear"
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
    ) -> Result<ChainId> {
        let id = ChainId(
            u32::try_from(self.chains.len())
                .map_err(|_| Error::Internal("a run's chain count cannot exceed u32"))?,
        );
        self.chains.push(Chain {
            ctx,
            frame: None,
            slice,
            index,
            addressed,
            block: 0,
            coroutine: None,
            incoming: None,
            reply: None,
            var: var.clone(),
            execute_depth,
            client: None,
            parent,
        });
        Ok(id)
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
            chain.execute_depth,
            chain.reply.as_deref(),
            &chain.client,
            &chain.var,
            VmSetupMode::Scheduler,
        )?;
        chain.frame = Some(frame);
        chain.block = 0;
        Ok(true)
    }

    /// Runs one ready chain to its next suspension point: resume a
    /// suspended coroutine with its delivered answer, or advance the walk -
    /// entering the next section, starting the next Lua block's coroutine,
    /// running one prose block inline, or falling through at a section's
    /// end.
    async fn step(&mut self, id: ChainId, root_result: &mut Option<Result<String>>) -> Result<()> {
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
            } else if chain.block >= chain.slice[chain.index].blocks.len() {
                Advance::SectionEnd
            } else {
                match &chain.slice[chain.index].blocks[chain.block] {
                    Block::Lua(_) => Advance::StartLua,
                    Block::Prose { .. } => Advance::RunProse,
                }
            }
        };
        match advance {
            Advance::EnterSection => self.advance_entry(id, root_result),
            Advance::Resume(thread, answer) => {
                let chain = &self.chains[id.index()];
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
                    .resume_block_coro_answer(program, &thread, answer);
                self.handle_coro_result(id, result, root_result)
            }
            Advance::StartLua => {
                let chain = &self.chains[id.index()];
                let slice = chain.slice;
                let program = match &slice[chain.index].blocks[chain.block] {
                    Block::Lua(program) => program,
                    Block::Prose { .. } => {
                        return Err(Error::Internal("the advance matched the block kind"));
                    }
                };
                let observer = Arc::clone(chain.ctx.observer());
                let execution = chain.ctx.execution().to_owned();
                let name = slice[chain.index].name.clone();
                let frame = chain
                    .frame
                    .as_ref()
                    .ok_or(Error::Internal("a live chain holds its frame"))?;
                // The driver owns the chunk observation boundaries: STARTED
                // at the block's start, SUCCEEDED or FAILED when its
                // coroutine finally returns or fails - a suspension is
                // neither.
                observer.observe(&execution, &name, detail::LUA_CHUNK_STARTED);
                let result = frame.vm()?.start_block_coro(program);
                self.handle_coro_result(id, result, root_result)
            }
            Advance::RunProse => {
                let chain = &mut self.chains[id.index()];
                let (text, loop_capable) = match &chain.slice[chain.index].blocks[chain.block] {
                    Block::Prose { text, loop_capable } => (text.clone(), *loop_capable),
                    Block::Lua(_) => {
                        return Err(Error::Internal("the advance matched the block kind"));
                    }
                };
                let name = chain.slice[chain.index].name.clone();
                let frame = chain
                    .frame
                    .as_mut()
                    .ok_or(Error::Internal("a live chain holds its frame"))?;
                frame
                    .run_prose_block(&chain.ctx, &name, &text, loop_capable, &mut chain.client)
                    .await?;
                chain.block += 1;
                self.ready.push_back(id);
                Ok(())
            }
            Advance::SectionEnd => {
                self.end_section(id)?;
                self.ready.push_back(id);
                Ok(())
            }
        }
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
        } else {
            // The walk ran off the slice's last section: the chain ends,
            // carrying its reply slot.
            self.finish(id, Ok(None), root_result);
        }
        Ok(())
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
        chain.index += 1;
        Ok(())
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
                chain.slice[chain.index].name.clone(),
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
                    Ok(request) => {
                        chain.coroutine = Some(thread);
                        self.dispatch(id, request)
                    }
                    Err(error) => {
                        observer.observe(&execution, &name, detail::LUA_CHUNK_FAILED);
                        Err(error)
                    }
                }
            }
            CoroStep::Done(LuaBlockResult::Jump(_)) => {
                // The chunk completed with a jump transfer marker, so the
                // chunk boundary reports success; the walk translation that
                // makes a jump a control transfer lands in a later step.
                observer.observe(&execution, &name, detail::LUA_CHUNK_SUCCEEDED);
                Err(Error::Lua(
                    "the scheduler does not walk jumps yet: the walk translation lands in a later step"
                        .to_owned(),
                ))
            }
            CoroStep::Done(LuaBlockResult::Returned(value)) => {
                observer.observe(&execution, &name, detail::LUA_CHUNK_SUCCEEDED);
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

    /// Dispatches one validated request from a suspended chain.
    ///
    /// # Errors
    /// Returns the typed protocol error for a received `fanout` or `mcp`
    /// request, which no call surface produces yet.
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
            Request::Fanout { .. } => Err(Request::fanout_reserved()),
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
                self.io_tasks.push(task.abort_handle());
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
                    section: chain.slice[chain.index].name.clone(),
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
        let section = chain.slice[chain.index].name.clone();
        let turns = Arc::clone(chain.ctx.turns());
        let request_id = RequestId(self.next_request);
        self.next_request += 1;
        let tx = self.answer_tx.clone();
        let task = tokio::task::spawn_local(async move {
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
        let depth = chain.execute_depth + 1;
        if depth > MAX_EXECUTE_DEPTH {
            return Err(Error::Lua(format!(
                "execute recursion exceeded cap of {MAX_EXECUTE_DEPTH}"
            )));
        }
        let slice = chain.slice;
        let index = chain.index;
        let args = input.unwrap_or_else(|| chain.ctx.args()).to_owned();
        let child_ctx = chain.ctx.with_args(&args);
        let client = chain.client.clone();
        // `chain`'s arena borrow ends here; `slice` borrows the prompt tree.
        let caller = &slice[index];
        let (child_slice, start) = match resolve_jump_target(target, slice, caller)? {
            JumpTarget::Child(child_index) => (caller.children.as_slice(), child_index),
            JumpTarget::Sibling(sibling_index) => (slice, sibling_index),
        };
        let child = self.start_chain(child_ctx, child_slice, start, true, Some(id), var, depth)?;
        // The child inherits the caller's client slot: an already-resolved
        // client is shared, an unresolved one stays lazy.
        self.chains[child.index()].client = client;
        Ok(child)
    }

    /// Finishes one chain: the frame's teardown boundary when the chain
    /// ends mid-section, then the outcome's delivery - the run's result for
    /// the root chain, the execute answer for a child chain.
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
        // `None` when the chain ended by exhausting its slice: the last
        // section's frame already dropped at the fall-through.
        let mut frame = chain.frame.take();
        let reply = chain.reply.clone();
        let outcome = outcome.and_then(|returned| {
            // A chain ending mid-section (a scalar return) reads its final
            // var back before teardown, exactly as a completed section does
            // at fall-through (the walk rolls it forward; an execute chain
            // discards its clone), and arms the completion flag so the
            // frame's drop fires SECTION_FINISHED. A failure - the
            // read-back's included - drops the frame unarmed.
            if let Some(frame) = frame.as_mut() {
                frame.read_var()?;
                frame.mark_completed();
            }
            let text = match returned {
                Some(value) => value,
                None => match reply {
                    Some(reply) => reply,
                    // The legacy mapping: the top-level chain falls back to
                    // the shared generic completion, an execute chain to the
                    // empty string.
                    None if parent.is_none() => GENERIC_COMPLETION.to_owned(),
                    None => String::new(),
                },
            };
            Ok(text)
        });
        // The frame drops here: the single teardown boundary.
        drop(frame);
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
