//! The request arms: one dispatch per validated protocol request from a
//! suspended chain. Every leaf arm builds its [`Effect`] and issues it
//! through the scheduler's one `issue` path; the arm performs nothing and
//! emits nothing for the answer, which `apply_answer` handles when it
//! lands. Every store operation is a leaf yield, handed to the caller
//! uniformly for all backends - no inline fast path - so interleaving
//! behavior never depends on which backend serves the mount.
//! The `tool_call`, `local_tool_done`, `chat`, `spawn`, `timer`,
//! `drain_task_notices`, and task wait, inspection, note, concurrency, and
//! cancel arms
//! are defined in their own modules.

use std::sync::Arc;

use crate::execute::protocol::{Answer, Request, VfsOp};
use crate::execute::run::Effect;
use crate::execute::section_context::TaskSeed;
use crate::execute::support::MAX_CALL_DEPTH;
use crate::lua::{ToolSet, resolve_model_binding};
use crate::model::Message;
use crate::model::ModelBinding;
use crate::{Error, Result};
use promptforge_types::event::ReplyOrigin;
use promptforge_types::event::lifecycle;
use promptforge_types::event::lifecycle::Lifecycle;
use promptforge_vfs::VfsError;

use super::{ChainIndex, Continuation, Counters, Scheduler, VfsContinuation};

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
/// nothing for an op this crate does not name.
fn vfs_observations(op: &VfsOp) -> Option<(Lifecycle, Lifecycle)> {
    let pair = match op {
        VfsOp::Write { .. } => (lifecycle::VFS_WRITE_SUCCEEDED, lifecycle::VFS_WRITE_FAILED),
        VfsOp::Append { .. } => (
            lifecycle::VFS_APPEND_SUCCEEDED,
            lifecycle::VFS_APPEND_FAILED,
        ),
        VfsOp::Read { .. } => (lifecycle::VFS_READ_SUCCEEDED, lifecycle::VFS_READ_FAILED),
        VfsOp::ReadNumbered { .. } => (
            lifecycle::VFS_READ_NUMBERED_SUCCEEDED,
            lifecycle::VFS_READ_NUMBERED_FAILED,
        ),
        VfsOp::StrReplace { .. } => (
            lifecycle::VFS_REPLACE_SUCCEEDED,
            lifecycle::VFS_REPLACE_FAILED,
        ),
        VfsOp::Delete { .. } => (
            lifecycle::VFS_DELETE_SUCCEEDED,
            lifecycle::VFS_DELETE_FAILED,
        ),
        VfsOp::Glob { .. } => (lifecycle::VFS_GLOB_SUCCEEDED, lifecycle::VFS_GLOB_FAILED),
        VfsOp::Exists { .. } => (
            lifecycle::VFS_EXISTS_SUCCEEDED,
            lifecycle::VFS_EXISTS_FAILED,
        ),
        // Any op `promptforge-lua` adds behind its `#[non_exhaustive]`
        // `VfsOp` before this crate names it reports nothing.
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
        Request::JoinAny { .. } => Some("tasks"),
        Request::ToolCall { .. } => Some("tool_call"),
        Request::Store { .. } => Some("store"),
        Request::Spawn { .. }
        | Request::Timer { .. }
        | Request::Ready { .. }
        | Request::Status { .. }
        | Request::Pending { .. }
        | Request::Concurrency { .. }
        | Request::Note { .. }
        | Request::Cancel { .. }
        | Request::DrainTaskNotices
        | Request::LocalToolDone { .. } => None,
    }
}

/// Classifies one store operation's failure for the answer channel. A
/// claims-model conflict becomes the fatal determinism violation: the
/// driver intercepts it at the answer boundary and ends the run on the
/// spot rather than resuming it into Lua, so no author `pcall` can catch
/// it. Every other failure returns as the call's answer as an
/// [`Error::Store`] holding the model-facing message rendered for the
/// operation and the structured cause, which ends the run as a
/// `RunErrorKind::Vfs` error if it aborts the chunk uncaught.
pub(super) fn classify_vfs_failure(op: &VfsOp, error: &VfsError) -> Error {
    if let VfsError::Conflict { detail, .. } = error {
        return Error::Determinism(detail.clone());
    }
    Error::store_op(op, error.clone())
}

impl Scheduler {
    /// Dispatches one validated request from a suspended chain.
    ///
    /// # Errors
    /// Returns the store arm's error when the chain's access capability is
    /// gone, or the `local_tool_done` arm's error when no local tool call is
    /// open.
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
                    var,
                    origin,
                    fanout,
                );
                Ok(())
            }
            Request::Timer { seconds } => {
                self.dispatch_timer(id, seconds);
                Ok(())
            }
            Request::JoinAny { tasks } => {
                self.dispatch_join_any(id, tasks);
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
            Request::Concurrency { limit } => {
                self.dispatch_concurrency(id, limit);
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
            Request::DrainTaskNotices => {
                self.dispatch_drain_task_notices(id);
                Ok(())
            }
            Request::ToolCall {
                alias,
                args,
                call_id,
                turn,
            } => {
                self.dispatch_tool_call(id, &alias, args, call_id, turn);
                Ok(())
            }
            Request::LocalToolDone { outcome } => self.dispatch_local_tool_done(id, outcome),
            Request::Store { op } => self.dispatch_vfs(id, op),
            Request::Chat { list, binding } => {
                self.dispatch_chat(id, &list, binding);
                Ok(())
            }
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
        // its `Infer` origin tells the caller its live deltas have no
        // consumer.
        let round = self.number_round(ReplyOrigin::Infer);
        let effect = Effect::Chat {
            options: binding.completion_options(),
            binding,
            messages: vec![Message::user(prompt)],
            after: None,
            keep: 0,
            tools: Vec::new(),
            round,
        };
        self.issue(id, effect, Continuation::Infer(round.id));
        Ok(())
    }

    /// Dispatches a `store` request: derives the store view from the
    /// chain's access capability and issues the operation through it as a
    /// `Vfs` effect for the caller to perform, parking the chain in the
    /// pending table exactly as a leaf I/O round does. Every store
    /// operation takes this yield path uniformly (memory mounts and real
    /// files alike, with no inline fast path) so interleaving behavior never
    /// depends on which backend serves the mount. The operation's event is
    /// pushed when the answer is applied, before the chain resumes.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the live chain's access capability
    /// is gone, which only the chain-end paths take, or [`Error::Store`]
    /// when the handle the chain's access came from declares no store,
    /// which the run's start probe already ruled out.
    fn dispatch_vfs(&mut self, id: ChainIndex, op: VfsOp) -> Result<()> {
        let access = Arc::clone(self.chains[id.index()].access()?);
        let view = Arc::new(promptforge_vfs::detail::store_view(&access).map_err(Error::store)?);
        let observations = vfs_observations(&op);
        let continuation = VfsContinuation {
            op: op.clone(),
            observations,
        };
        let effect = Effect::Vfs { access: view, op };
        self.issue(id, effect, Continuation::Vfs(continuation));
        Ok(())
    }

    /// Dispatches a `call` request: constructs the child chain and
    /// enqueues it; the parent blocks until the child's finish delivers
    /// its final text as the answer. Every dispatch
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
            Ok(child) => self.ready.push_back(child),
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
        // A call chain's effective limit starts with its caller's: the
        // tasks it spawns run within the caller's share.
        let child_concurrency = chain.concurrency;
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
            var.clone(),
            depth,
            child_concurrency,
        )?;
        self.chains[child.index()].access = access;
        Ok(child)
    }
}
