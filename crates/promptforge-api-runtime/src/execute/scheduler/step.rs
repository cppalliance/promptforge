//! One chain's step to its next suspension point: resume a suspended
//! coroutine with its delivered answer, or advance the walk - entering the
//! next section, starting the next Lua block's coroutine, stashing one
//! prose block as the pending Markdown buffer, or falling through at a
//! section's end - then apply the block coroutine's outcome: park a
//! yielded chain on its request's dispatch, advance or finish a completed
//! block, and report the chunk's closing observation boundary.

use std::sync::Arc;

use mlua::Thread;

use crate::execute::protocol::{Answer, YieldParse};
use crate::lua::{CoroStep, LuaBlockResult};
use crate::observe::detail;
use crate::parser::Block;
use crate::{Error, Result, cancel};

use super::{ChainIndex, Scheduler};

impl Scheduler<'_> {
    /// Runs one ready chain to its next suspension point. An arm chain's
    /// step runs inside the arm's own cancel scope: the handle is the
    /// run's, cloned at dispatch, so the scope re-installs the same
    /// task-local the driver already runs under - the per-arm wiring the
    /// legacy engine needed a spawn boundary crossing for (PF-CANCEL-002).
    pub(super) async fn step(
        &mut self,
        id: ChainIndex,
        root_result: &mut Option<Result<String>>,
    ) -> Result<()> {
        let cancel = self.chains[id.index()]
            .arm
            .as_ref()
            .and_then(|arm| arm.cancel.clone());
        // The step body never awaits: every dispatch either spawns its leaf
        // work or answers on the spot. The scope exists so a dispatch arm
        // that captures the current cancel handle (the `tool_call` arm
        // hands it to its spawned task) sees the arm's handle.
        cancel::maybe_scope(cancel, async move { self.step_inner(id, root_result) }).await
    }

    /// Runs one ready chain to its next suspension point: resume a
    /// suspended coroutine with its delivered answer, or advance the walk -
    /// entering the next section, starting the next Lua block's coroutine,
    /// stashing one prose block as the pending Markdown buffer, or falling
    /// through at a section's end.
    fn step_inner(
        &mut self,
        id: ChainIndex,
        root_result: &mut Option<Result<String>>,
    ) -> Result<()> {
        /// What the chain does next, decided under the chain borrow so the
        /// action phase can touch the scheduler's other fields.
        enum Advance {
            /// Resume the suspended coroutine with its delivered answer.
            Resume(Thread, Answer<Error>),
            /// The chain is between sections: enter the next section, or
            /// end the chain when the slice is exhausted.
            EnterSection,
            /// Start the current Lua block as a fresh coroutine.
            StartLua,
            /// Stash the current prose block as the pending Markdown buffer
            /// the next Lua fence consumes.
            StashProse,
            /// The section's blocks are exhausted: fall through.
            SectionEnd,
        }
        let advance = {
            let chain = &mut self.chains[id.index()];
            if let Some(answer) = chain.incoming.take() {
                let Some(thread) = chain.coroutine.take() else {
                    return Err(Error::internal(
                        "a delivered answer implies a suspended coroutine",
                    ));
                };
                // The answer ends whatever the chain was parked on.
                chain.blocked = None;
                Advance::Resume(thread, answer)
            } else if chain.coroutine.is_some() {
                return Err(Error::internal(
                    "a ready chain's suspended coroutine waits on its answer",
                ));
            } else if chain.frame.is_none() {
                Advance::EnterSection
            } else if chain.block >= chain.blocks().len() {
                Advance::SectionEnd
            } else {
                match &chain.blocks()[chain.block] {
                    Block::Lua(_) => Advance::StartLua,
                    Block::Prose { .. } => Advance::StashProse,
                    // `Block` is `#[non_exhaustive]` across the crate seam; a
                    // future variant has no advance rule yet.
                    _ => {
                        return Err(Error::internal("an unrecognized block kind cannot advance"));
                    }
                }
            }
        };
        match advance {
            Advance::EnterSection => self.advance_entry(id, root_result),
            Advance::Resume(thread, answer) => self.resume_block(id, &thread, answer, root_result),
            Advance::StartLua => self.start_lua(id, root_result),
            Advance::StashProse => {
                let chain = &mut self.chains[id.index()];
                let text = match &chain.blocks()[chain.block] {
                    Block::Prose { text, .. } => text.clone(),
                    _ => {
                        return Err(Error::internal("the advance matched the block kind"));
                    }
                };
                // The parser emits one prose block per inter-fence gap,
                // already accumulated and reset at thematic breaks, so the
                // block IS the pending buffer the next Lua fence consumes.
                // Prose never infers: the buffer waits for the following
                // block's lazy `prose` install, unevaluated until read.
                chain.pending_prose = Some(text);
                chain.block += 1;
                self.ready.push_back(id);
                Ok(())
            }
            Advance::SectionEnd => {
                if self.chains[id.index()].h1.is_some() {
                    self.end_live_h1(id, root_result, 0)?;
                } else {
                    self.end_section(id)?;
                    self.ready.push_back(id);
                }
                Ok(())
            }
        }
    }

    /// Resumes a chain's suspended coroutine with its delivered answer.
    fn resume_block(
        &mut self,
        id: ChainIndex,
        thread: &Thread,
        answer: Answer<Error>,
        root_result: &mut Option<Result<String>>,
    ) -> Result<()> {
        let chain = &self.chains[id.index()];
        let Block::Lua(program) = &chain.blocks()[chain.block] else {
            return Err(Error::internal("a suspended coroutine's block is Lua"));
        };
        let frame = chain
            .frame
            .as_ref()
            .ok_or(Error::internal("a live chain holds its frame"))?;
        let result = frame
            .vm()?
            .resume_block_coro_answer(program, thread, answer);
        self.handle_coro_result(id, result, root_result)
    }

    /// Starts the chain's current Lua block as a fresh coroutine: the
    /// pending Markdown buffer installs as the block's fresh read-only
    /// lazy `prose` template first. The
    /// driver owns the chunk observation
    /// boundaries: STARTED at the block's start, SUCCEEDED or FAILED when
    /// its coroutine finally returns or fails - a suspension is neither.
    fn start_lua(
        &mut self,
        id: ChainIndex,
        root_result: &mut Option<Result<String>>,
    ) -> Result<()> {
        let pending = self.chains[id.index()].pending_prose.take();
        let chain = &self.chains[id.index()];
        let observer = Arc::clone(chain.ctx.observer());
        let execution = chain.ctx.execution().to_owned();
        let name = chain.section_name().to_owned();
        observer.observe(&execution, &name, detail::LUA_CHUNK_STARTED);
        let frame = chain
            .frame
            .as_ref()
            .ok_or(Error::internal("a live chain holds its frame"))?;
        if let Err(error) = frame.install_lazy_prose(&chain.ctx, pending.as_deref().unwrap_or("")) {
            observer.observe(&execution, &name, detail::LUA_CHUNK_FAILED);
            return Err(error);
        }
        let Block::Lua(program) = &chain.blocks()[chain.block] else {
            return Err(Error::internal("the advance matched the block kind"));
        };
        let result = frame.vm()?.start_block_coro(program).map_err(Error::from);
        self.handle_coro_result(id, result, root_result)
    }

    /// Applies one Lua block coroutine's outcome: parks a yielded chain on
    /// its request's dispatch, advances or finishes a completed block, and
    /// reports the chunk's closing observation boundary.
    fn handle_coro_result(
        &mut self,
        id: ChainIndex,
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
                // A failed H1 assertion ends the run before the walk:
                // H1's remaining job is the prompt's hard gates, so the
                // prompt chunk's own Lua failure IS the failed assertion
                // and its message is the failure notice. Only the chunk's
                // error remaps: the machinery around it (the shared
                // replay, the final `var` read-back, jump-target
                // resolution) keeps its own kind - a prompt bug under
                // `Error::Lua`, not an unsatisfiable environment. Fatal
                // run conditions (cancellation, the claims violation)
                // keep their own classification either way.
                let error = if self.chains[id.index()].h1.is_some() {
                    match error {
                        Error::Lua(_) | Error::LuaRuntime { .. } => Error::RequirementsUnmet {
                            notice: error.to_string(),
                        },
                        other => other,
                    }
                } else {
                    error
                };
                return Err(error);
            }
        };
        match step {
            CoroStep::Yielded(thread, values) => {
                let chain = &mut self.chains[id.index()];
                let frame = chain
                    .frame
                    .as_ref()
                    .ok_or(Error::internal("a live chain holds its frame"))?;
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
                        chain.incoming = Some(answer.map_error(Error::from));
                        self.ready.push_back(id);
                        Ok(())
                    }
                    YieldParse::Malformed(error) => {
                        observer.observe(&execution, &name, detail::LUA_CHUNK_FAILED);
                        Err(Error::from(error))
                    }
                }
            }
            CoroStep::Done(LuaBlockResult::Jump(heading)) => {
                // A jump is a control transfer, not a failure: the chunk
                // boundary reports success and the walk moves to the
                // resolved target. A jump out of H1 ends the pass and
                // starts the walk at the target.
                observer.observe(&execution, &name, detail::LUA_CHUNK_SUCCEEDED);
                if self.chains[id.index()].h1.is_some() {
                    return self.end_live_h1_at_jump(id, &heading, root_result);
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
                        // A scalar return from the H1 pass
                        // short-circuits the whole run. The final `var`
                        // read-back runs here exactly as the walk
                        // reads it on every exit, so a reassigned `var`
                        // global fails the run instead of returning the
                        // value; the frame then drops unarmed - the pass
                        // never fires SECTION_FINISHED.
                        let mut frame = chain
                            .frame
                            .take()
                            .ok_or(Error::internal("a live chain holds its frame"))?;
                        frame.read_var()?;
                        drop(frame);
                        // The run ends here, so the pass's live tasks end
                        // under the chain-end rules as at any chain end.
                        *root_result = Some(self.settle_owned_tasks(id, Ok(value)));
                        return Ok(());
                    }
                    // H1 does not read the `reply` global back after a
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
                let chain = &mut self.chains[id.index()];
                chain.block += 1;
                self.ready.push_back(id);
                Ok(())
            }
        }
    }
}
