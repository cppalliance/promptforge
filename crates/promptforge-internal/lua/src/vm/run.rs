//! Section VM execution: block coroutines, from their start through each
//! resume to the classified end.

use mlua::thread::ThreadStatus;
use mlua::{IntoLuaMulti, MultiValue, Thread, Value};

use super::SectionVm;
use crate::coro::{block_guard, take_failure};
use crate::error::{Error, Result};
use crate::error_value::ErrorKind;
use crate::handles::LuaBlockResult;
use crate::program::LuaProgram;
use crate::protocol::{Answer, Request, Retained, YieldParse};

impl SectionVm {
    /// Takes any recorded jump target, propagating a poisoned jump-slot lock
    /// rather than silently coercing the failure into "no jump".
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the jump-slot mutex is poisoned.
    pub(crate) fn take_jump(&self) -> Result<Option<String>> {
        let mut slot = self
            .jump_slot
            .lock()
            .map_err(|_| Error::Internal(JUMP_SLOT_POISONED.into()))?;
        Ok(slot.take())
    }

    /// Clears any jump target an earlier chunk recorded.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the jump-slot mutex is poisoned.
    pub(crate) fn clear_jump(&self) -> Result<()> {
        self.take_jump().map(drop)
    }

    /// Starts one Lua block as a coroutine on this VM and resumes it to its
    /// first yield or its end.
    ///
    /// This is the scheduler's chunk-execution path: one coroutine per Lua
    /// block, created from the block's loaded function on this persistent
    /// VM, so the VM's globals (`var` and the bare globals) roll forward
    /// across blocks. Instruction hooks are
    /// per-coroutine in PUC Lua, so the VM's budget/cancellation hook is
    /// installed on the fresh thread; the main-state hook from construction
    /// never fires inside a resumed coroutine.
    ///
    /// A coroutine that returns ends the block: a recorded jump takes
    /// precedence over the chunk's own error, genuine failures map through
    /// `LuaProgram::map_runtime_error_among` with the VM's loaded library
    /// programs, and the scalar-return rule applies to the return values.
    /// A coroutine that yields suspends with the shim's request table; the
    /// driver resumes it with
    /// [`resume_block_coro`](Self::resume_block_coro). No observation
    /// events fire here; the driver owns the chunk observation boundaries.
    ///
    /// The coroutine body is the shim's block guard, resumed first with
    /// the block function: the guard runs the block under `xpcall`, whose
    /// handler stashes a failure and its raise-point traceback, and
    /// re-raises the same value, so a structured error table a shim raised
    /// reaches the Engine as a typed [`Error::Raised`] rather than only as
    /// mlua's stringification, and a Lua-raised error keeps the author's
    /// frames for the prompt-line mapping.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] if the jump slot is poisoned, or
    /// [`Error::LuaRuntime`] if the program cannot load, the shim prelude
    /// never ran on this VM, or the thread cannot be created or hooked; a
    /// block failure returns the mapped runtime error.
    pub fn start_block_coro(&self, program: &LuaProgram) -> Result<CoroStep> {
        self.clear_jump()?;
        let function = program.load(&self.lua)?;
        let guard = block_guard(&self.lua)?;
        let thread = self.lua.create_thread(guard).map_err(Error::lua)?;
        self.instruction_budget.install_on_thread(&thread)?;
        let result = thread.resume::<MultiValue>(function);
        self.step_block_coro::<Error>(program, thread, result, None)
    }

    /// Resumes a suspended block coroutine with the driver's answer values.
    ///
    /// # Errors
    /// Same contract as [`start_block_coro`](Self::start_block_coro).
    pub fn resume_block_coro(
        &self,
        program: &LuaProgram,
        thread: &Thread,
        args: impl IntoLuaMulti,
    ) -> Result<CoroStep> {
        let result = thread.resume::<MultiValue>(args);
        self.step_block_coro::<Error>(program, thread.clone(), result, None)
    }

    /// Validates a suspended block coroutine's yielded values into the
    /// boundary outcome.
    ///
    /// A shim yields exactly its request table; anything else is
    /// [`YieldParse::Malformed`] and fails the block as a hand-rolled or
    /// corrupted yield. Author code cannot reach
    /// `coroutine.yield` (the global is stripped at shim install), so this
    /// strict validation is defense in depth. A well-formed call whose
    /// argument fails validation is [`YieldParse::Call`]: the error is
    /// returned as the answer so the shim raises it at the call site.
    #[must_use]
    pub fn request_from_yield(&self, values: &MultiValue) -> YieldParse {
        Request::from_yield(&self.lua, values.iter().next().unwrap_or(&Value::Nil))
    }

    /// Resumes a suspended block coroutine with the driver's answer.
    ///
    /// The answer renders to its `(ok, result)` envelope on this VM. On a
    /// failure answer the envelope includes the error's structured table
    /// (`kind`, `message`, fields) for the shim to raise. When the block's
    /// failure is a raise of that very table - the shim's own raise, or the
    /// block's re-raise of the value it caught - the typed error the answer
    /// owned is substituted back, so the Rust caller receives the
    /// structured error rather than a string. A block that caught the
    /// table and failed with anything else, its message text included,
    /// keeps its own error.
    ///
    /// The error type is the driver's own (`E`); this crate's internal
    /// failures convert into it through [`From`], and its
    /// [`ErrorValue`](crate::ErrorValue) rendering supplies the table's
    /// kind.
    ///
    /// # Errors
    /// Same contract as [`start_block_coro`](Self::start_block_coro), plus
    /// the driver's `E` if the envelope cannot be rendered on this VM.
    pub fn resume_block_coro_answer<E>(
        &self,
        program: &LuaProgram,
        thread: &Thread,
        answer: Answer<E>,
    ) -> std::result::Result<CoroStep, E>
    where
        E: crate::ErrorValue + From<Error>,
    {
        let (envelope, retained) = answer.render(&self.lua).map_err(Error::lua)?;
        let result = thread.resume::<MultiValue>(envelope);
        self.step_block_coro(program, thread.clone(), result, retained)
    }

    fn step_block_coro<E: From<Error>>(
        &self,
        program: &LuaProgram,
        thread: Thread,
        result: mlua::Result<MultiValue>,
        retained: Option<Retained<E>>,
    ) -> std::result::Result<CoroStep, E> {
        match result {
            // A resumed thread that is still resumable suspended on a yield;
            // the yielded values are the shim's request table.
            Ok(values) if thread.status() == ThreadStatus::Resumable => {
                Ok(CoroStep::Yielded(thread, values))
            }
            result => {
                // A recorded jump takes precedence over the chunk's error,
                // which is the jump's own transfer marker.
                if let Some(heading) = self.take_jump()? {
                    return Ok(CoroStep::Done(LuaBlockResult::Jump(heading)));
                }
                let returned = match result {
                    Ok(returned) => returned,
                    Err(error) => return Err(self.block_failure(program, &error, retained)?),
                };
                Ok(CoroStep::Done(LuaBlockResult::Returned(scalar_return(
                    returned,
                )?)))
            }
        }
    }

    /// Classifies a block coroutine's failure: the guard's stash restores
    /// the raise-point traceback onto a Lua-raised error first (the guard's
    /// re-raise is what killed the coroutine, so mlua's own traceback shows
    /// only the guard's frame), and the failure maps through
    /// [`map_chunk_failure`](Self::map_chunk_failure) to the mapped prompt
    /// line. Cancellation and Engine quotas keep that mapping. Otherwise a
    /// raise of the answer's retained error table is the retained typed
    /// error, and any other structured error table the guard stashed is
    /// kept as [`Error::Raised`], except a `lua`-kind table, whose mapped
    /// runtime error keeps the same message with its source and the
    /// mapped prompt line. The stash is taken on every failure so it never
    /// goes stale.
    fn block_failure<E: From<Error>>(
        &self,
        program: &LuaProgram,
        error: &mlua::Error,
        retained: Option<Retained<E>>,
    ) -> Result<E> {
        let stashed = take_failure(&self.lua)?;
        let mapped = self.map_chunk_failure(program, &stashed.restore_traceback(error));
        if let Error::Interrupted | Error::LuaQuota { .. } = mapped {
            return Ok(E::from(mapped));
        }
        if let Some(retained) = retained
            && stashed.raised_table_is(&retained.table)
        {
            return Ok(retained.error);
        }
        Ok(E::from(match stashed.raised {
            None => mapped,
            Some(raised) => match raised.kind {
                ErrorKind::Lua => mapped,
                ErrorKind::ToolLoopExhausted
                | ErrorKind::ContextExhausted
                | ErrorKind::EmptyModelReply
                | ErrorKind::OutOfScopeTool
                | ErrorKind::UnboundTool
                | ErrorKind::Tool
                | ErrorKind::TaskNotOwned
                | ErrorKind::TaskConsumed
                | ErrorKind::TasksLive
                | ErrorKind::Cancelled
                | ErrorKind::Store
                | ErrorKind::Internal => Error::Raised(raised),
            },
        }))
    }

    /// Maps one chunk's Lua failure to its typed outcome: a chunk the
    /// instruction hook aborted under the run's cancel flag is
    /// [`Error::Interrupted`], whatever the raw error says; everything
    /// else maps through `LuaProgram::map_runtime_error_among` of `program`
    /// and of every library block `require` loaded into this VM.
    pub(crate) fn map_chunk_failure(&self, program: &LuaProgram, error: &mlua::Error) -> Error {
        if self.instruction_budget.is_cancelled() {
            return Error::Interrupted;
        }
        match self.library_programs.lock() {
            Ok(libraries) => program.map_runtime_error_among(error, &libraries),
            Err(_) => Error::Internal("require's loaded library programs were poisoned".into()),
        }
    }
}

/// The failure of a poisoned jump slot.
const JUMP_SLOT_POISONED: &str = "jump slot poisoned";

/// Renders a returned Lua scalar as the section's result string. Tables and other
/// non-scalar returns are errors.
fn value_to_string(value: &Value) -> Result<String> {
    match value {
        Value::String(s) => Ok(s.to_string_lossy()),
        Value::Integer(i) => Ok(i.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Boolean(b) => Ok(b.to_string()),
        other => Err(Error::Lua(format!(
            "cannot return a {} as a result",
            other.type_name()
        ))),
    }
}

/// Converts only the first returned Lua value into an optional scalar string.
///
/// Later values are ignored. A missing or nil first value returns `None`.
///
/// # Errors
/// Returns [`Error::Lua`] when the first value is not a supported scalar.
pub(crate) fn scalar_return(returned: MultiValue) -> Result<Option<String>> {
    match returned.into_iter().next() {
        None | Some(Value::Nil) => Ok(None),
        Some(value) => value_to_string(&value).map(Some),
    }
}

/// One step of a Lua block running on a coroutine.
///
/// Produced by [`SectionVm::start_block_coro`] and
/// [`SectionVm::resume_block_coro`]; consumed by the scheduler's driver,
/// which validates a [`Yielded`](CoroStep::Yielded) request, dispatches it,
/// and resumes the thread with the answer.
#[derive(Debug)]
pub enum CoroStep {
    /// The coroutine suspended on a shim yield; the yielded values are
    /// the request table.
    Yielded(Thread, MultiValue),
    /// The coroutine ended (return, jump, or error); a recorded jump wins
    /// over the chunk's own error, and the scalar-return rule applies.
    Done(LuaBlockResult),
}
