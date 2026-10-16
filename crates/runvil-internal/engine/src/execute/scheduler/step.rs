//! One chain's step to its next suspension point: resume a suspended
//! coroutine with its delivered answer, or advance the walk - entering the
//! next section, starting the next Lua block's coroutine, stashing one
//! prose block as the pending Markdown buffer, or falling through at a
//! section's end - then apply the block coroutine's outcome: park a
//! yielded chain on its request's dispatch, advance or finish a completed
//! block, and report the chunk's closing observation boundary.

use std::sync::Arc;

use mlua::Thread;
use runvil_types::event::lifecycle;

use crate::execute::protocol::{Answer, YieldParse};
use crate::lua::{CoroStep, LuaBlockResult};
use crate::parser::{Block, BodyText};
use crate::{Error, Result};

use super::chain::ChainStep;
use super::chain_record::SectionIndex;
use super::{ChainIndex, Scheduler};

/// The notice a failed H1 gate ends the run with; the chunk's own failure
/// follows it as the cause.
const H1_GATE_FAILED: &str = "the environment cannot satisfy this prompt:\n- the H1 gate failed";

/// What a ready chain does next, decided under the chain borrow so the
/// action phase can touch the scheduler's other fields.
#[derive(Debug)]
enum Advance {
    /// Resumes the suspended coroutine with its delivered answer.
    Resume(Thread, Answer<Error>),
    /// The chain is between sections: enter the next section, or end the
    /// chain when the slice is exhausted.
    EnterSection,
    /// Starts the current Lua block as a fresh coroutine.
    StartLua,
    /// Stashes the current prose block as the pending Markdown buffer the
    /// next Lua fence consumes.
    StashProse(BodyText),
    /// The section's blocks are exhausted: fall through.
    SectionEnd,
}

impl Scheduler {
    /// Runs one ready chain to its next suspension point: resume a
    /// suspended coroutine with its delivered answer, or advance the walk -
    /// entering the next section, starting the next Lua block's coroutine,
    /// stashing one prose block as the pending Markdown buffer, or falling
    /// through at a section's end. The step never awaits: every dispatch
    /// either issues its leaf effect or answers on the spot.
    pub(super) fn step_chain(&mut self, id: ChainIndex) -> Result<ChainStep> {
        let prompt = self.prompt();
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
            } else {
                match chain.current_block(&prompt) {
                    None => Advance::SectionEnd,
                    Some(Block::Lua(_)) => Advance::StartLua,
                    Some(Block::Prose { text, .. }) => Advance::StashProse(text.clone()),
                    // `Block` is `#[non_exhaustive]` across the crate seam; an
                    // unlisted variant has no advance rule.
                    Some(_) => {
                        return Err(Error::internal("an unrecognized block kind cannot advance"));
                    }
                }
            }
        };
        match advance {
            Advance::EnterSection => self.advance_entry(id),
            Advance::Resume(thread, answer) => self.resume_block(id, &thread, answer),
            Advance::StartLua => self.start_lua(id),
            Advance::StashProse(text) => {
                // The parser emits one prose block per inter-fence gap,
                // already accumulated and reset at thematic breaks, so the
                // block IS the pending buffer the next Lua fence consumes.
                // Prose never infers: the buffer waits for the following
                // block's lazy `prose` install, unevaluated until read.
                let chain = &mut self.chains[id.index()];
                chain.pending_prose = Some(text);
                chain.block = chain.block.next();
                self.ready.push_back(id);
                Ok(ChainStep::Continue)
            }
            Advance::SectionEnd => {
                if self.chains[id.index()].kind.is_h1() {
                    return self.end_live_h1(id, SectionIndex::FIRST);
                }
                self.end_section(id)?;
                self.ready.push_back(id);
                Ok(ChainStep::Continue)
            }
        }
    }

    /// Resumes a chain's suspended coroutine with its delivered answer.
    fn resume_block(
        &mut self,
        id: ChainIndex,
        thread: &Thread,
        answer: Answer<Error>,
    ) -> Result<ChainStep> {
        let prompt = self.prompt();
        let chain = &self.chains[id.index()];
        let Some(Block::Lua(program)) = chain.current_block(&prompt) else {
            return Err(Error::internal("a suspended coroutine's block is Lua"));
        };
        let result = chain
            .frame()?
            .vm()?
            .resume_block_coro_answer(program, thread, answer);
        self.handle_coro_result(id, result)
    }

    /// Starts the chain's current Lua block as a fresh coroutine: the
    /// pending Markdown buffer installs as the block's fresh read-only
    /// lazy `prose` template first. The
    /// driver owns the chunk observation
    /// boundaries: STARTED at the block's start, SUCCEEDED or FAILED when
    /// its coroutine finally returns or fails - a suspension is neither.
    fn start_lua(&mut self, id: ChainIndex) -> Result<ChainStep> {
        let prompt = self.prompt();
        let pending = self.chains[id.index()].pending_prose.take();
        let chain = &self.chains[id.index()];
        let emitter = Arc::clone(chain.ctx.emitter());
        let name = chain.section_name().to_owned();
        emitter.report(&name, lifecycle::LUA_CHUNK_STARTED);
        let frame = chain.frame()?;
        if let Err(error) =
            frame.install_lazy_prose(&chain.ctx, pending.as_ref().map_or("", BodyText::as_str))
        {
            emitter.report(&name, lifecycle::LUA_CHUNK_FAILED);
            return Err(error);
        }
        let Some(Block::Lua(program)) = chain.current_block(&prompt) else {
            return Err(Error::internal("the advance matched the block kind"));
        };
        let result = frame.vm()?.start_block_coro(program).map_err(Error::from);
        self.handle_coro_result(id, result)
    }

    /// Applies one Lua block coroutine's outcome: parks a yielded chain on
    /// its request's dispatch, advances or finishes a completed block, and
    /// reports the chunk's closing observation boundary.
    fn handle_coro_result(
        &mut self,
        id: ChainIndex,
        result: Result<CoroStep>,
    ) -> Result<ChainStep> {
        let (emitter, name) = {
            let chain = &self.chains[id.index()];
            (
                Arc::clone(chain.ctx.emitter()),
                chain.section_name().to_owned(),
            )
        };
        let step = match result {
            Ok(step) => step,
            Err(error) => {
                emitter.report(&name, lifecycle::LUA_CHUNK_FAILED);
                // A failed H1 assertion ends the run before the walk:
                // H1's remaining job is the prompt's hard gates, so the
                // prompt chunk's own Lua failure IS the failed assertion,
                // kept whole as the refusal's cause. Only the chunk's
                // error remaps: the machinery around it (section setup,
                // the final `var` read-back, jump-target resolution)
                // keeps its own kind - a prompt bug under
                // `Error::Lua`, not an unsatisfiable environment. Fatal
                // run conditions (cancellation, the claims violation)
                // keep their own classification either way.
                if !self.chains[id.index()].kind.is_h1() {
                    return Err(error);
                }
                return Err(match error {
                    error @ (Error::Lua(_) | Error::LuaRuntime { .. }) => {
                        Error::RequirementsUnmet {
                            notice: H1_GATE_FAILED.to_owned(),
                            cause: Some(Box::new(error)),
                        }
                    }
                    other => other,
                });
            }
        };
        match step {
            CoroStep::Yielded(thread, values) => {
                let chain = &mut self.chains[id.index()];
                match chain.frame()?.vm()?.request_from_yield(&values) {
                    YieldParse::Request(request) => {
                        chain.coroutine = Some(thread);
                        self.dispatch(id, request)?;
                        Ok(ChainStep::Continue)
                    }
                    YieldParse::Call(answer) => {
                        // An argument-validation failure is the call's
                        // answer: the shim raises it at the call site, so
                        // an author `pcall` catches it.
                        chain.coroutine = Some(thread);
                        self.answer_inline(id, answer.map_error(Error::from));
                        Ok(ChainStep::Continue)
                    }
                    YieldParse::Malformed(error) => {
                        emitter.report(&name, lifecycle::LUA_CHUNK_FAILED);
                        Err(Error::from(error))
                    }
                }
            }
            CoroStep::Done(LuaBlockResult::Jump(heading)) => {
                // A jump is a control transfer, not a failure: the chunk
                // boundary reports success and the walk moves to the
                // resolved target. A jump out of H1 ends the pass and
                // starts the walk at the target.
                emitter.report(&name, lifecycle::LUA_CHUNK_SUCCEEDED);
                if self.chains[id.index()].kind.is_h1() {
                    return self.end_live_h1_at_jump(id, &heading);
                }
                self.apply_jump(id, &heading)?;
                self.ready.push_back(id);
                Ok(ChainStep::Continue)
            }
            CoroStep::Done(LuaBlockResult::Returned(value)) => {
                emitter.report(&name, lifecycle::LUA_CHUNK_SUCCEEDED);
                let chain = &mut self.chains[id.index()];
                match value {
                    Some(value) if chain.kind.is_h1() => {
                        // A scalar return from the H1 pass
                        // short-circuits the whole run. The final `var`
                        // read-back runs here exactly as the walk
                        // reads it on every exit, so a reassigned `var`
                        // global fails the run instead of returning the
                        // value; the frame then drops unarmed - the pass
                        // never fires SECTION_FINISHED.
                        chain.frame()?.read_var()?;
                        chain.frame = None;
                        // The run ends here, so the pass's live tasks end
                        // under the chain-end rules as at any chain end.
                        Ok(ChainStep::RunEnded(self.settle_owned_tasks(id, Ok(value))))
                    }
                    // A scalar return ends the chain it fired in.
                    Some(value) => Ok(self.finish(id, Ok(Some(value)))),
                    None => {
                        chain.block = chain.block.next();
                        self.ready.push_back(id);
                        Ok(ChainStep::Continue)
                    }
                }
            }
        }
    }
}
