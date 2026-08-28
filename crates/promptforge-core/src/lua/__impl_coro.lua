-- Coroutine-protocol shim prelude for a scheduler-mode section VM.
--
-- The host installs this after the host tables exist and before the shared
-- library replays. The chunk arguments are privileged captures, never
-- globals: `yield` is coroutine.yield (the coroutine global is stripped
-- after install, so author code cannot yield directly), `var_snapshot` is
-- the host helper returning the hidden `var` data table as a plain deep
-- copy, and `models` is the section's models table, passed in so the chunk
-- never reads a global.
local yield, var_snapshot, models = ...

-- The (ok, result) envelope: level 0 suppresses the position prefix, so a
-- shim-raised error carries exactly the host's message.
local function infer(prompt)
  local ok, result = yield({ op = "infer", prompt = prompt })
  if not ok then error(result, 0) end
  return result
end

-- A model handle reaches author code as a proxy table: field reads pass
-- through to the inner userdata, and infer is a Lua method that yields with
-- the inner userdata attached (a Rust userdata method cannot yield).
local function wrap_handle(handle)
  -- __metatable seals the proxy: getmetatable survives hardening, so an
  -- unprotected metatable would hand the inner userdata (and its
  -- non-yielding Rust infer method) back to author code.
  local proxy = setmetatable({}, { __index = handle, __metatable = false })
  function proxy.infer(_, prompt, opts)
    if opts ~= nil then
      error("model:infer(prompt) does not accept a second argument; per-call inference options are not supported", 0)
    end
    local ok, result = yield({ op = "infer", prompt = prompt, handle = handle })
    if not ok then error(result, 0) end
    return result
  end
  return proxy
end

local function execute_section(target, input)
  local ok, result = yield({
    op = "execute",
    target = target,
    input = input,
    var = var_snapshot(),
  })
  if not ok then error(result, 0) end
  return result
end

models.infer = infer
local raw_use, raw_get = models.use, models.get
models.use = function(alias) return wrap_handle(raw_use(alias)) end
models.get = function(alias) return wrap_handle(raw_get(alias)) end

return {
  execute = execute_section,
  wrap_handle = wrap_handle,
}
