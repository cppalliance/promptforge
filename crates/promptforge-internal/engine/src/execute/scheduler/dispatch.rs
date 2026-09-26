//! The request arms: one dispatch per validated protocol request from a
//! suspended chain. Every leaf arm builds its [`Effect`] and issues it
//! through the scheduler's one `issue` path; the arm performs nothing and
//! emits nothing for the answer, which `apply_answer` handles when it
//! lands. Every store operation is a leaf yield, handed to the host
//! uniformly for all backends - no inline fast path - so interleaving
//! behavior never depends on which backend serves the mount.
//! A received `mcp` request is the protocol's typed reserved error. The
//! `tool_call`, `local_tool_done`, `chat`, `spawn`, `timer`, `task_events`,
//! `drain_task_notices`, and task wait, inspection, note, and cancel arms
//! are defined in their own modules.

use std::sync::Arc;

use crate::execute::protocol::{Answer, Request, StoreOp};
use crate::execute::run::Effect;
use crate::execute::section_context::TaskSeed;
use crate::execute::support::MAX_CALL_DEPTH;
use crate::lua::{ToolSet, resolve_model_binding};
use crate::model::Message;
use crate::model::ModelBinding;
use crate::store::StoreError;
use crate::{Error, Result};
use promptforge_types::event::lifecycle;
use promptforge_types::event::lifecycle::Lifecycle;

use super::{ChainIndex, Continuation, Counters, Scheduler};

/// The error for an alias that names no binding in the run's tool catalog:
/// the name and every bound alias, so the message reads required versus
/// actual. Shared by the script `tool_call` arm and the `chat` arm's
/// explicit tool list.
pub(super) fn unbound_tool_call(tool_set: &ToolSet, name: &str) -> Error {
    Error::UnboundToolCall {
        name: name.to_owned(),
        bound: tool_set
            .bindings()
            .iter()
            .map(|binding| binding.alias().to_owned())
            .collect(),
    }
}

/// The succeeded/failed observation pair one store operation reports;
/// `exists` reports nothing.
fn store_observations(op: &StoreOp) -> Option<(Lifecycle, Lifecycle)> {
    let pair = match op {
        StoreOp::Write { .. } => (
            lifecycle::STORE_WRITE_SUCCEEDED,
            lifecycle::STORE_WRITE_FAILED,
        ),
        StoreOp::Append { .. } => (
            lifecycle::STORE_APPEND_SUCCEEDED,
            lifecycle::STORE_APPEND_FAILED,
        ),
        StoreOp::Read { .. } => (
            lifecycle::STORE_READ_SUCCEEDED,
            lifecycle::STORE_READ_FAILED,
        ),
        StoreOp::ReadNumbered { .. } => (
            lifecycle::STORE_READ_NUMBERED_SUCCEEDED,
            lifecycle::STORE_READ_NUMBERED_FAILED,
        ),
        StoreOp::StrReplace { .. } => (
            lifecycle::STORE_REPLACE_SUCCEEDED,
            lifecycle::STORE_REPLACE_FAILED,
        ),
        StoreOp::Delete { .. } => (
            lifecycle::STORE_DELETE_SUCCEEDED,
            lifecycle::STORE_DELETE_FAILED,
        ),
        StoreOp::Glob { .. } => (
            lifecycle::STORE_GLOB_SUCCEEDED,
            lifecycle::STORE_GLOB_FAILED,
        ),
        // `exists` reports nothing, and so does any op `promptforge-lua`
        // adds behind its `#[non_exhaustive]` `StoreOp` before this crate
        // names it.
        _ => return None,
    };
    Some(pair)
}

/// What a chain parks on when it yields `request`, as `tasks.status`
/// reports it; `None` for the arms answered inline, whose chain is back on
/// the ready queue before anyone can look.
fn blocked_on(request: &Request) -> Option<&'static str> {
    match request {
        Request::Infer { .. } | Request::Chat { .. } => Some("chat"),
        Request::Call { .. } => Some("call"),
        Request::WhenAny { .. } | Request::TaskEvents { .. } => Some("tasks"),
        Request::ToolCall { .. } => Some("tool_call"),
        Request::UserInput => Some("user_input"),
        Request::Store { .. } => Some("store"),
        Request::Spawn { .. }
        | Request::Timer { .. }
        | Request::Ready { .. }
        | Request::Status { .. }
        | Request::Pending { .. }
        | Request::Note { .. }
        | Request::Cancel { .. }
        | Request::DrainTaskNotices
        | Request::LocalToolDone { .. }
        | Request::Mcp { .. } => None,
    }
}

/// Classifies one store operation's failure for the answer channel. A
/// claims-model conflict becomes the fatal determinism violation: the
/// driver intercepts it at the answer boundary and ends the run on the
/// spot rather than resuming it into Lua, so no author `pcall` can catch
/// it. Every other failure returns as the call's answer with the store's
/// own message, classified `Lua` if it aborts the chunk uncaught.
pub(super) fn classify_store_failure(error: &StoreError) -> Error {
    if let Some(detail) = error.conflict_detail() {
        return Error::Determinism(detail.to_owned());
    }
    Error::Lua(error.to_string())
}

impl Scheduler {
    /// Dispatches one validated request from a suspended chain.
    ///
    /// # Errors
    /// Returns the typed protocol error for a received `mcp` request, which
    /// no call surface produces yet, the store arm's error when the
    /// chain's access capability is gone, or the `local_tool_done` arm's
    /// error when no local tool call is open.
    pub(super) fn dispatch(&mut self, id: ChainIndex, request: Request) -> Result<()> {
        self.chains[id.index()].blocked = blocked_on(&request);
        match request {
            Request::Infer { prompt, binding } => {
                self.dispatch_infer(id, &prompt, binding);
                Ok(())
            }
            Request::Call { target, input, var } => {
                self.dispatch_call(id, &target, input.as_deref(), &var);
                Ok(())
            }
            Request::Spawn {
                target,
                input,
                item,
                index,
                var,
                origin,
                fanout,
            } => {
                self.dispatch_spawn(
                    id,
                    &target,
                    input.as_deref(),
                    TaskSeed { item, index },
                    &var,
                    origin,
                    fanout,
                );
                Ok(())
            }
            Request::Timer { seconds } => {
                self.dispatch_timer(id, seconds);
                Ok(())
            }
            Request::WhenAny { tasks } => {
                self.dispatch_when_any(id, tasks);
                Ok(())
            }
            Request::Ready { task } => {
                self.dispatch_ready(id, &task);
                Ok(())
            }
            Request::Status { task } => {
                self.dispatch_status(id, &task);
                Ok(())
            }
            Request::Pending { origin } => {
                self.dispatch_pending(id, origin);
                Ok(())
            }
            Request::Note { text } => {
                self.dispatch_note(id, text);
                Ok(())
            }
            Request::Cancel { task } => {
                self.dispatch_cancel(id, &task);
                Ok(())
            }
            Request::TaskEvents { task, last } => {
                self.dispatch_task_events(id, &task, last);
                Ok(())
            }
            Request::DrainTaskNotices => {
                self.dispatch_drain_task_notices(id);
                Ok(())
            }
            Request::ToolCall {
                alias,
                args,
                call_id,
            } => {
                self.dispatch_tool_call(id, &alias, args, call_id);
                Ok(())
            }
            Request::LocalToolDone { outcome } => self.dispatch_local_tool_done(id, outcome),
            Request::UserInput => {
                self.dispatch_user_input(id);
                Ok(())
            }
            Request::Store { op } => self.dispatch_store(id, op),
            Request::Chat { messages, binding } => {
                self.dispatch_chat(id, &messages, binding);
                Ok(())
            }
            Request::Mcp { .. } => Err(Error::from(Request::mcp_reserved())),
        }
    }

    /// Dispatches an `infer` request: resolves the binding, issues the
    /// single tool-free gateway round as a `Chat` effect over one user
    /// message, and parks the chain in the pending table. A resolution
    /// failure is the call's answer, resumed into the caller so an author
    /// `pcall` can catch it.
    fn dispatch_infer(&mut self, id: ChainIndex, prompt: &str, binding: Option<ModelBinding>) {
        if let Err(error) = self.issue_infer(id, prompt, binding) {
            self.answer_inline(id, Answer::Infer(Err(error)));
        }
    }

    /// The fallible half of infer dispatch: the binding resolution (the
    /// handle's frozen binding, else the section's current model) and the
    /// issued effect.
    fn issue_infer(
        &mut self,
        id: ChainIndex,
        prompt: &str,
        binding: Option<ModelBinding>,
    ) -> Result<()> {
        let chain = &self.chains[id.index()];
        let binding = if let Some(binding) = binding {
            binding
        } else {
            let frame = chain
                .frame
                .as_ref()
                .ok_or(Error::internal("a live chain holds its frame"))?;
            resolve_model_binding(chain.ctx.models(), &frame.vm()?.model_runtime)?.ok_or_else(
                || Error::ModelRequired {
                    section: chain.section_name().to_owned(),
                },
            )?
        };
        // A nested infer round consumes only the accumulated completion;
        // live deltas have no consumer here.
        let effect = Effect::Chat {
            options: binding.completion_options(),
            binding,
            messages: vec![Message::user(prompt)],
            tools: Vec::new(),
            stream: false,
        };
        self.issue(id, effect, Continuation::Infer);
        Ok(())
    }

    /// Dispatches a `user_input` request: the host answers the issued
    /// `UserInput` effect exactly as a leaf I/O round does, so a blocking
    /// wait parks its chain - the section's VM and message history intact -
    /// without blocking the run, and a cancel drops it with every other
    /// outstanding effect. Every request is issued whether or not the host
    /// has an operator to ask: a host without one answers with the
    /// unavailable-fallback policy (the fixed fallback sentence with
    /// `available` false). The wait is reported here; a delivered response
    /// is reported when its answer is applied; an unavailable answer
    /// records no input.
    fn dispatch_user_input(&mut self, id: ChainIndex) {
        let chain = &self.chains[id.index()];
        let execution = chain.ctx.execution().to_owned();
        let section = chain.section_name().to_owned();
        chain
            .ctx
            .emitter()
            .report(&section, lifecycle::USER_INPUT_WAIT_STARTED);
        let effect = Effect::UserInput { execution, section };
        self.issue(id, effect, Continuation::UserInput);
    }

    /// Dispatches a `store` request: issues the operation under the
    /// chain's access capability as a `Store` effect for the host to
    /// perform, parking the chain in the pending table exactly as a leaf
    /// I/O round does. Every store operation takes this yield path
    /// uniformly (memory- and host-backed alike, with no inline fast path)
    /// so interleaving behavior never depends on which backend serves
    /// the mount. The operation's event is pushed when the answer is
    /// applied, before the chain resumes.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the live chain's access capability
    /// is gone, which only the chain-end paths take.
    fn dispatch_store(&mut self, id: ChainIndex, op: StoreOp) -> Result<()> {
        let access = Arc::clone(self.chains[id.index()].access()?);
        let observations = store_observations(&op);
        let effect = Effect::Store { access, op };
        self.issue(id, effect, Continuation::Store(observations));
        Ok(())
    }

    /// Dispatches a `call` request: constructs the child chain, pushes
    /// it on the chain stack, and enqueues it; the parent blocks until the
    /// child's finish delivers its final text as the answer. Every dispatch
    /// failure - the depth cap, target resolution, child construction - is
    /// the call's answer, resumed into the caller so an author `pcall` can
    /// catch it.
    fn dispatch_call(
        &mut self,
        id: ChainIndex,
        target: &str,
        input: Option<&str>,
        var: &serde_json::Value,
    ) {
        match self.prepare_call(id, target, input, var) {
            Ok(child) => {
                self.stack.push(child);
                self.ready.push_back(child);
            }
            Err(error) => {
                self.answer_inline(id, Answer::Call(Err(error)));
            }
        }
    }

    /// The fallible half of call dispatch: the depth cap checked against
    /// the caller's call-depth field, the target resolved over the
    /// caller's visible set, and the child chain constructed one level
    /// deeper under the call's args and `var` snapshot.
    fn prepare_call(
        &mut self,
        id: ChainIndex,
        target: &str,
        input: Option<&str>,
        var: &serde_json::Value,
    ) -> Result<ChainIndex> {
        let chain = &self.chains[id.index()];
        let depth = chain.call_depth + 1;
        if depth > MAX_CALL_DEPTH {
            return Err(Error::Lua(format!(
                "call recursion exceeded cap of {MAX_CALL_DEPTH}"
            )));
        }
        // An explicit input forks the chain's args (and `argv` re-derives
        // from them); a no-input call inherits the caller's context whole,
        // so the run's frozen `argv` - H1's repair included - reaches the
        // chain rather than re-deriving from the unchanged args.
        let child_ctx = match input {
            Some(input) => chain.ctx.with_args(input),
            None => chain.ctx.clone(),
        };
        // A call chain is a blocking child: it borrows the caller's access
        // capability (the same serial thread of execution), so the caller's
        // standing claims never false-conflict with the child's ops.
        let access = chain.access.clone();
        // `chain`'s arena borrow ends here; the resolution names the
        // target's slice by path, so nothing borrows the arena across it.
        let target_section = self.resolve_chain_target(id, target)?;
        // The child's id is the caller's next child index: `call` children
        // and spawned tasks share the caller's counter, so the id depends
        // only on the caller's own dispatch order.
        let chain_id = self.allocate_child_id(id)?;
        let child = self.start_chain(
            chain_id,
            Counters::default(),
            child_ctx,
            target_section.slice,
            target_section.index,
            Some(id),
            var,
            depth,
        )?;
        self.chains[child.index()].access = access;
        Ok(child)
    }
}
