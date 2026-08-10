use super::*;

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
        globals
            .set(name, Value::Nil)
            .map_err(|e| Error::Lua(e.to_string()))?;
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
    .map_err(|e| Error::Lua(e.to_string()))?;
    Ok(())
}

/// Install an instruction-count hook that aborts a runaway block.
pub(crate) fn install_instruction_budget(lua: &Lua) {
    let fired = Arc::new(AtomicU64::new(0));
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
        move |_lua, _debug| {
            // Cooperative cancellation: abort a long-running Lua block promptly
            // when the run's CancelHandle is signaled (mapped to
            // Error::Interrupted at the runtime-error boundary).
            if crate::cancel::is_cancelled() {
                return Err(mlua::Error::RuntimeError(
                    "lua execution cancelled".to_string(),
                ));
            }
            if fired.fetch_add(1, Ordering::Relaxed) >= HOOK_BUDGET {
                return Err(mlua::Error::RuntimeError(
                    crate::error::lua_quota::INSTRUCTION.to_string(),
                ));
            }
            Ok(VmState::Continue)
        },
    );
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

pub(crate) fn scalar_return(returned: MultiValue) -> Result<Option<String>> {
    match returned.into_iter().next() {
        None | Some(Value::Nil) => Ok(None),
        Some(value) => value_to_string(&value).map(Some),
    }
}

#[cfg(test)]
mod tests;
