//! Section VM execution: the shared-library replay, direct chunk runs,
//! and block coroutines, from their start through each resume to the
//! classified end.

use super::SectionVm;
use crate::protocol::{Answer, Request, YieldParse};
#[cfg(test)]
use crate::{Access, Arc, GuardNonce, Json};
use crate::{
    Emitter, Error, IntoLuaMulti, LuaBlockResult, LuaProgram, MultiValue, Result, Thread,
    ThreadStatus, Value, block_guard, lifecycle, scalar_return, take_failure,
};

impl SectionVm {
    /// Replays the shared library as the section's first chunk.
    ///
    /// The replay runs through the normal chunk path with every Engine
    /// global already installed: `args`, `sys`, `var`, `log`,
    /// `store`, the `tools`/`models` tables, and the control globals are all
    /// visible to shared top-level code. Only the captured tool/model alias
    /// globals are absent; they install afterward via
    /// [`install_captured_bindings`](Self::install_captured_bindings) so a
    /// declared alias wins over a same-named shared global. A scalar
    /// top-level return is discarded because the replay is a library load.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the shared program fails or returns a
    /// non-scalar value, or if it calls `jump`: load-time control transfer
    /// has no coherent meaning, so a recorded jump becomes the hard error
    /// "jump is not available during shared library load".
    pub fn replay_shared(
        &self,
        program: &LuaProgram,
        emitter: &Emitter,
        section: &str,
    ) -> Result<()> {
        emitter.report(section, lifecycle::LUA_SHARED_LOAD_STARTED);
        match self.run_loaded_with_control(program) {
            Ok(LuaBlockResult::Returned(_)) => {
                emitter.report(section, lifecycle::LUA_SHARED_LOAD_SUCCEEDED);
                Ok(())
            }
            Ok(LuaBlockResult::Jump(_)) => {
                emitter.report(section, lifecycle::LUA_SHARED_LOAD_FAILED);
                Err(Error::Lua(
                    "jump is not available during shared library load".to_owned(),
                ))
            }
            Err(error) => {
                emitter.report(section, lifecycle::LUA_SHARED_LOAD_FAILED);
                Err(error)
            }
        }
    }

    /// Executes a compiled Lua chunk in this VM's persistent environment.
    ///
    /// This is the direct, scheduler-free path for running a section's Lua
    /// blocks; the scheduler drives blocks through
    /// [`start_block_coro`](Self::start_block_coro) instead. Store and
    /// `log` reports go to the emitter captured by
    /// [`install_host_apis`](Self::install_host_apis); a nil or absent
    /// top-level return produces [`LuaBlockResult::Returned`]`(None)`. When
    /// the chunk may call `call`, `jump`, or `fanout`, those must
    /// already be installed by `install_control_globals`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if Engine values have not been injected, execution
    /// fails, or the program returns a non-scalar value.
    ///
    /// A test helper for `promptforge-engine`'s executor tests, so it exists
    /// only under `test-support`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn run_chunk(
        &self,
        program: &LuaProgram,
        emitter: &Emitter,
        section: &str,
    ) -> Result<LuaBlockResult> {
        emitter.report(section, lifecycle::LUA_CHUNK_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            emitter.report(section, lifecycle::LUA_CHUNK_FAILED);
            return Err(error);
        }
        let result = self.run_loaded_with_control(program);
        emitter.report(
            section,
            if result.is_ok() {
                lifecycle::LUA_CHUNK_SUCCEEDED
            } else {
                lifecycle::LUA_CHUNK_FAILED
            },
        );
        result
    }

    /// Takes any recorded jump target, propagating a poisoned jump-slot lock
    /// rather than silently coercing the failure into "no jump".
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the jump-slot mutex is poisoned.
    fn take_jump(&self) -> Result<Option<String>> {
        let mut slot = self
            .jump_slot
            .lock()
            .map_err(|_| Error::Lua("jump slot poisoned".to_owned()))?;
        Ok(slot.take())
    }

    fn run_loaded_with_control(&self, program: &LuaProgram) -> Result<LuaBlockResult> {
        {
            let mut slot = self
                .jump_slot
                .lock()
                .map_err(|_| Error::Lua("jump slot poisoned".to_owned()))?;
            *slot = None;
        }
        let result = program.load(&self.lua)?.call(());
        // A recorded jump takes precedence over the chunk's error: that error
        // is the jump's own transfer marker, not a real failure. A poisoned
        // slot propagates rather than coercing into "no jump".
        if let Some(heading) = self.take_jump()? {
            return Ok(LuaBlockResult::Jump(heading));
        }
        let returned = result.map_err(|error| self.map_chunk_failure(program, &error))?;
        Ok(LuaBlockResult::Returned(scalar_return(returned)?))
    }

    /// Starts one Lua block as a coroutine on this VM and resumes it to its
    /// first yield or its end.
    ///
    /// This is the scheduler's chunk-execution path: one coroutine per Lua
    /// block, created from the block's loaded function on this persistent
    /// VM, so the VM's globals (`var`, the bare globals, the captured
    /// handles) roll forward across blocks as on the direct
    /// [`run_chunk`](Self::run_chunk) path. Instruction hooks are
    /// per-coroutine in PUC Lua, so the VM's budget/cancellation hook is
    /// installed on the fresh thread; the main-state hook from construction
    /// never fires inside a resumed coroutine.
    ///
    /// A coroutine that returns ends the block under the direct path's
    /// contract: a recorded jump takes precedence over the chunk's own
    /// error, genuine failures map through
    /// [`LuaProgram::map_runtime_error`], and the scalar-return rule
    /// applies to the return values. A coroutine that yields suspends with
    /// the shim's request table; the driver resumes it with
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
    /// Returns [`Error::Lua`] if the jump slot is poisoned, the program
    /// cannot load, the shim prelude never ran on this VM, or the thread
    /// cannot be created or hooked; a block failure returns the mapped
    /// runtime error.
    pub fn start_block_coro(&self, program: &LuaProgram) -> Result<CoroStep> {
        {
            let mut slot = self
                .jump_slot
                .lock()
                .map_err(|_| Error::Lua("jump slot poisoned".to_owned()))?;
            *slot = None;
        }
        let function = program.load(&self.lua)?;
        let guard = block_guard(&self.lua)?;
        let thread = self.lua.create_thread(guard).map_err(Error::lua)?;
        self.instruction_budget.install_on_thread(&thread)?;
        let result = thread.resume::<MultiValue>(function);
        self.step_block_coro(program, thread, result)
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
        self.step_block_coro(program, thread.clone(), result)
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
    /// (`kind`, `message`, fields) for the shim to raise, and the typed
    /// error the answer owned is substituted back when the shim-raised
    /// error surfaces as the coroutine's failure, so the Rust caller
    /// receives the structured error rather than a string.
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
        let (envelope, retained) = answer.into_envelope(&self.lua).map_err(Error::lua)?;
        match self.resume_block_coro(program, thread, envelope) {
            Ok(step) => Ok(step),
            Err(error) => Err(match retained {
                Some(retained) if coroutine_failure_is(&error, &retained) => retained,
                _ => E::from(error),
            }),
        }
    }

    fn step_block_coro(
        &self,
        program: &LuaProgram,
        thread: Thread,
        result: mlua::Result<MultiValue>,
    ) -> Result<CoroStep> {
        match result {
            // A resumed thread that is still resumable suspended on a yield;
            // the yielded values are the shim's request table.
            Ok(values) if thread.status() == ThreadStatus::Resumable => {
                Ok(CoroStep::Yielded(thread, values))
            }
            result => {
                // A recorded jump takes precedence over the chunk's error,
                // exactly as on the direct path.
                if let Some(heading) = self.take_jump()? {
                    return Ok(CoroStep::Done(LuaBlockResult::Jump(heading)));
                }
                let returned = match result {
                    Ok(returned) => returned,
                    Err(error) => return Err(self.block_failure(program, &error)?),
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
    /// only the guard's frame); cancellation and Engine quotas then map
    /// through [`LuaProgram::map_runtime_error`]; otherwise a structured
    /// error table the guard stashed is kept as [`Error::Raised`], except a
    /// `lua`-kind table, whose mapped runtime error keeps the same
    /// message with its source and the mapped prompt line. The stash is
    /// taken on every failure so it never goes stale.
    fn block_failure(&self, program: &LuaProgram, error: &mlua::Error) -> Result<Error> {
        let stashed = take_failure(&self.lua)?;
        let mapped = self.map_chunk_failure(program, &stashed.restore_traceback(error));
        if matches!(mapped, Error::Interrupted | Error::LuaQuota { .. }) {
            return Ok(mapped);
        }
        Ok(match stashed.raised {
            Some(raised) if raised.kind != crate::ErrorKind::Lua => Error::Raised(raised),
            _ => mapped,
        })
    }

    /// Maps one chunk's Lua failure to its typed outcome: a chunk the
    /// instruction hook aborted under the run's cancel flag is
    /// [`Error::Interrupted`], whatever the raw error says; everything
    /// else maps through [`LuaProgram::map_runtime_error`].
    fn map_chunk_failure(&self, program: &LuaProgram, error: &mlua::Error) -> Error {
        if self.instruction_budget.is_cancelled() {
            return Error::Interrupted;
        }
        program.map_runtime_error(error)
    }
}

/// Whether a block coroutine's failure is the shim's re-raise of the
/// answer's retained typed error: the shim raises `error(result, 0)` on the
/// error table, whose `tostring` is the message, so the inner `mlua`
/// runtime message's first line (or the kept table's message) is exactly
/// the retained error's display. The comparison reads the retained `mlua`
/// source rather than the mapped message, whose `Display` includes mlua's
/// `runtime error: ` prefix. A block that caught the shim's error and
/// failed on its own keeps its own error.
fn coroutine_failure_is<E: std::fmt::Display>(failure: &Error, retained: &E) -> bool {
    let display = retained.to_string();
    match failure {
        Error::LuaRuntime { source, .. } => match source.downcast_ref::<mlua::Error>() {
            Some(mlua::Error::RuntimeError(message)) => {
                message.lines().next() == Some(display.as_str())
            }
            _ => false,
        },
        Error::Lua(message) => message.lines().next() == Some(display.as_str()),
        Error::Raised(raised) => raised.message.lines().next() == Some(display.as_str()),
        _ => false,
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
    /// The coroutine ended (return, jump, or error); the block outcome
    /// follows the `run_loaded_with_control` contract.
    Done(LuaBlockResult),
}

/// The result of running a section's Lua block.
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct LuaOutcome {
    /// The chunk's top-level return value, if it returned one (the finish case).
    pub(crate) returned: Option<String>,
    /// The `var` table after the block ran, as JSON, for prose substitution.
    pub(crate) var: Json,
}

/// Runs a section's Lua chunk with `args` and `sys` exposed, a writable `var`
/// table available, and a `store` table backed by `store`, returning the
/// chunk's return value and the final `var`. Harness-mediated store operations
/// report safe outcomes through `emitter` under `section`.
/// `log(message)` reports constrained author checkpoints through the same
/// emitter; direct `print` is unavailable.
///
/// `store` is the run-scoped virtual-file handle; every section in a run is
/// given the same handle, so files a section writes persist for later sections
/// even though each section starts a fresh context. The exposed `store` table
/// is always present (an Engine global, not a scoped tool).
///
/// The `tools` table is the same validating one every section VM installs,
/// over an empty shared set: a chunk that calls `tools.add(...)` fails loudly
/// because no alias is bound.
///
/// # Errors
/// Returns [`Error::Lua`] if the sandbox cannot be built, `sys`/`var`/`store`
/// cannot be bridged, the chunk fails to run (including a failing `store` op,
/// which raises a Lua error), or it returns a value that cannot be rendered
/// as a result string.
#[cfg(test)]
pub(crate) fn run_chunk(
    source: &str,
    args: &str,
    sys: &Json,
    access: &Arc<Access>,
    emitter: &Emitter,
    section: &str,
) -> Result<LuaOutcome> {
    let mut vm = SectionVm::new(&GuardNonce::from_seed(0), emitter, section)?;
    vm.inject_host(args, sys, access)?;
    vm.install_host_apis(emitter, section)?;
    let returned: MultiValue = vm.lua.load(source).eval().map_err(Error::lua)?;
    let returned = scalar_return(returned)?;
    let var = vm.var()?;

    Ok(LuaOutcome { returned, var })
}
