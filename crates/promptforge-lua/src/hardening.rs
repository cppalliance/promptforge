use super::{
    Arc, AtomicU64, Error, HOOK_BUDGET, HOOK_INTERVAL, HookTriggers, Lua, MultiValue, Ordering,
    Result, Thread, Value, VmState,
};

/// Remove code-loading, direct output, and reflection globals the base library
/// provides. The `io`, `os`, `package`, `coroutine`, and `debug` libraries are
/// never loaded.
///
/// Also wraps `table.concat` so userdata with `__tostring` (fanout result
/// objects) coerce like `tostring`, keeping existing `table.concat(results)`
/// callers working after fanout returns structured objects. Tables and
/// booleans still error as stock Lua would.
pub(crate) fn harden(lua: &Lua) -> Result<()> {
    let globals = lua.globals();
    for name in [
        "load",
        "loadstring",
        "dofile",
        "loadfile",
        "collectgarbage",
        "require",
        "getfenv",
        "setfenv",
        "rawget",
        "rawset",
        "rawequal",
        "rawlen",
        "print",
        "warn",
    ] {
        globals.set(name, Value::Nil).map_err(Error::lua)?;
    }
    lua.load(
        r#"
local orig = table.concat
function table.concat(list, sep, i, j)
  i = i or 1
  j = j or #list
  local parts = {}
  for k = i, j do
    local v = list[k]
    local ty = type(v)
    if ty == "string" or ty == "number" then
      parts[#parts + 1] = v
    elseif ty == "userdata" then
      -- Fanout result objects (and any other userdata with __tostring).
      -- mlua metatables are not readable via getmetatable, so type-gate here.
      parts[#parts + 1] = tostring(v)
    elseif v == nil then
      error("invalid value (nil) at index " .. k .. " in table for 'concat'")
    else
      error("invalid value (" .. ty .. ") at index " .. k .. " in table for 'concat'")
    end
  end
  return orig(parts, sep)
end
"#,
    )
    .exec()
    .map_err(Error::lua)?;
    Ok(())
}

/// A VM's shared instruction-hook state.
///
/// The hook exists for cooperative cancellation: its trip budget
/// ([`HOOK_BUDGET`]) is effectively unlimited, so a long-running or infinite
/// loop is legal and only the run's cancel flag aborts it.
///
/// Instruction hooks are per-coroutine in PUC Lua: the hook installed on the
/// main state at construction never fires inside a resumed coroutine, so
/// every block coroutine receives the same hook (over the same counter) via
/// [`install_on_thread`](Self::install_on_thread), keeping every chunk the
/// VM runs cancellable.
#[derive(Debug, Default, Clone)]
pub(crate) struct InstructionBudget {
    fired: Arc<AtomicU64>,
}

impl InstructionBudget {
    /// Installs the budget/cancellation hook on a block coroutine.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the VM rejects the hook callback.
    pub(crate) fn install_on_thread(&self, thread: &Thread) -> Result<()> {
        thread
            .set_hook(
                HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
                budget_hook(Arc::clone(&self.fired)),
            )
            .map_err(Error::lua)
    }
}

/// The every-Nth-instruction hook body shared by the main state and every
/// block coroutine: the cancellation poll, plus a trip budget that is
/// effectively unlimited ([`HOOK_BUDGET`]) and never fires in practice.
fn budget_hook(
    fired: Arc<AtomicU64>,
) -> impl Fn(&Lua, &mlua::debug::Debug) -> mlua::Result<VmState> {
    move |_lua, _debug| {
        // Cooperative cancellation: abort a long-running Lua block promptly
        // when the run's CancelHandle is signaled (mapped to
        // Error::Interrupted at the runtime-error boundary).
        if promptforge_core_support::cancel::is_cancelled() {
            return Err(mlua::Error::RuntimeError(
                "lua execution cancelled".to_string(),
            ));
        }
        if fired.fetch_add(1, Ordering::Relaxed) == HOOK_BUDGET {
            return Err(mlua::Error::RuntimeError(
                crate::error::lua_quota::INSTRUCTION.to_string(),
            ));
        }
        Ok(VmState::Continue)
    }
}

/// Install the every-Nth-instruction hook that keeps a block cancellable.
///
/// The hook covers the main state only; coroutines need
/// [`InstructionBudget::install_on_thread`] with the returned counter.
///
/// # Errors
/// Returns [`Error::Lua`] if the VM rejects the hook callback.
pub(crate) fn install_instruction_budget(lua: &Lua) -> Result<InstructionBudget> {
    let budget = InstructionBudget::default();
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
        budget_hook(Arc::clone(&budget.fired)),
    )
    .map_err(Error::lua)?;
    Ok(budget)
}

/// Render a returned Lua scalar as the section's result string. Tables and other
/// non-scalar returns are deferred to a later commit.
pub(crate) fn value_to_string(value: &Value) -> Result<String> {
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
