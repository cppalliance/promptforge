-- Coroutine-protocol shim prelude for a scheduler-mode section VM.
--
-- The Engine installs this after the Engine globals exist and before the
-- shared library replays. The chunk arguments are privileged captures, never
-- globals: `yield` is coroutine.yield (the coroutine global is stripped
-- after install, so author code cannot yield directly), `var_snapshot` is
-- the Engine helper returning the hidden `var` data table as a plain deep
-- copy, `models`/`tools` are the section's namespace tables, passed in so
-- the chunk never reads a global, `error_value` builds the structured
-- error table (`{ kind, message, ... }` under the Engine's shared
-- metatable, whose `__tostring` is `message`), `stash_failure` records a
-- block's raised value for the Engine before the guard re-raises it, and
-- `normalize_failure` turns a Rust callback's raised failure (mlua's
-- opaque userdata) into the error table, passing every other value
-- through unchanged, `enter_local_handler()` and `leave_local_handler()`
-- step the counter the Engine's `jump` reads, so `jump` refuses while a
-- local tool's handler runs, and `cancel_requested()` reports whether the
-- run's cancel flag is set: a failure caught under cancellation is the
-- instruction hook's abort, which must unwind to the block guard, so every
-- protected call below raises it again instead of returning it,
-- `is_model_handle(value)` reports whether a value is a model handle, and
-- `loop_begin(entry, ...)` starts one `models.loop` call in Rust over the
-- run's round cap and the section's `compactors` table, returning the
-- call's step function and its first action. The `tasks` namespace and
-- the `fanout` shim live in their own chunks (`__impl_tasks.lua`,
-- `__impl_fanout.lua`), installed by the Engine right after this one over
-- the failure helpers this chunk returns.
local yield, var_snapshot, models, tools, error_value, stash_failure,
  normalize_failure, enter_local_handler, leave_local_handler,
  cancel_requested, is_model_handle, loop_begin = ...

-- The base library's pcall and xpcall, captured before the replacements
-- below are installed over the globals: the block guard needs the raw
-- failure value so the Engine's runtime-error mapping keeps its source.
local raw_pcall, raw_xpcall = pcall, xpcall

-- error, captured so author code that rebinds `error` cannot move a raise.
local error = error

-- math.type, captured at install for the same reason: the shim's type
-- names must not move when author code rebinds `math`.
local math_type = math.type

-- The Engine's name for a value's type, as the protocol parse reports it:
-- Lua folds integers and floats into "number", while the Engine names an
-- integer "integer" and a float "number", so a shim-raised argument error
-- reads exactly as the parse-raised one for the same value.
local function engine_type(value)
  if math_type(value) == "integer" then return "integer" end
  return type(value)
end

-- Every failure that reaches author code is one error table: `tostring`
-- gives exactly the message, and a caller that branches reads `kind` and
-- the kind's fields. Level 0 suppresses the position prefix (a table never
-- gets one, but a string fallback would), so a shim-raised error shows
-- exactly the Engine's message.
local function raise(kind, fields)
  error(error_value(kind, fields), 0)
end

-- The (ok, result) envelope's failure path. The Engine renders its typed
-- error as the table already; a bare string (a hand-built envelope) is
-- normalized to a `lua`-kind table so the shape holds without exception.
local function fail(result)
  if type(result) == "table" then error(result, 0) end
  raise("lua", { message = tostring(result) })
end

-- pcall and xpcall, replacing the base library's over the globals so a
-- Engine function that fails directly from Rust (`tools.add`, `models.get`,
-- a `sys` or `var` guard) reaches author code as the same error table a
-- shim raise does, instead of mlua's opaque userdata that `err.kind`
-- cannot index. Only a Rust-raised failure is rewritten; a string, an
-- author's own table, and an error table already built pass through
-- untouched. The raw pcall is yieldable, and so is this Lua frame, so a
-- shim yield inside the protected function still suspends the block.
-- Under cancellation the raw failure is raised again, so an author loop
-- around pcall cannot outlive the run.
local function pcall_outcome(ok, ...)
  if ok then return true, ... end
  if cancel_requested() then error((...), 0) end
  return false, normalize_failure((...))
end

local function protected_call(f, ...)
  return pcall_outcome(raw_pcall(f, ...))
end

-- The message handler sees the normalized failure; a non-function handler
-- is left to the raw xpcall so its own argument error is unchanged. Under
-- cancellation the author's handler never runs and the raw failure is
-- raised again: the hook's abort calls the message handler while hooks
-- are off, so a looping handler could not be interrupted.
local function xpcall_outcome(ok, ...)
  if not ok and cancel_requested() then error((...), 0) end
  return ok, ...
end

local function protected_xcall(f, handler, ...)
  if type(handler) ~= "function" then
    return xpcall_outcome(raw_xpcall(f, handler, ...))
  end
  return xpcall_outcome(raw_xpcall(f, function(failure)
    if cancel_requested() then return failure end
    return handler(normalize_failure(failure))
  end, ...))
end

-- One infer round: on the handle's frozen binding when there is one, and
-- otherwise on the section's current model, which the driver resolves.
local function run_infer(handle, prompt)
  local ok, result = yield({ op = "infer", prompt = prompt, handle = handle })
  if not ok then fail(result) end
  return result
end

-- models.infer(prompt) and a model handle's `infer`, as `h:infer(prompt)`.
-- Handles carry `infer` and `loop` as fields that read these entries, Lua
-- functions because each may suspend and a Rust method cannot. Each entry
-- checks its receiver and then its arity, counting trailing nils, before
-- any yield.
local function infer(...)
  if is_model_handle((...)) then
    raise("lua", {
      message = "models.infer takes (prompt); call handle:infer(prompt) to run on a model handle",
    })
  end
  if select('#', ...) > 1 then
    raise("lua", { message = "models.infer takes (prompt)" })
  end
  return run_infer(nil, (...))
end

local function handle_infer(...)
  if not is_model_handle((...)) then
    raise("lua", { message = "call infer on a model handle with a colon: handle:infer(prompt)" })
  end
  if select('#', ...) > 2 then
    raise("lua", { message = "handle:infer takes (prompt)" })
  end
  local handle, prompt = ...
  return run_infer(handle, prompt)
end

local function call_section(target, input)
  local ok, result = yield({
    op = "call",
    target = target,
    input = input,
    var = var_snapshot(),
  })
  if not ok then fail(result) end
  return result
end

-- Runs a local tool's handler inside this block coroutine, so every
-- suspending call the handler makes is an ordinary yield of the chain.
-- `jump` refuses while the handler runs; the raw pcall catches every
-- failure, so the leave always follows the enter, and it keeps a Rust
-- callback's failure in mlua's own form.
-- The `local_tool_done` yield carries the handler's first return value
-- (only when it returned) for the driver to report; afterward the
-- handler's own failure is raised again unchanged, a rejected return
-- raises the driver's error, and a returned value resumes as its text.
-- A failure under cancellation is raised at once, with no yield.
local function run_local_tool(handler, args)
  enter_local_handler()
  local ok, value = raw_pcall(handler, args)
  leave_local_handler()
  if not ok and cancel_requested() then error(value, 0) end
  local done = { op = "local_tool_done", ok = ok }
  if ok then done.value = value end
  local answered, result = yield(done)
  if not ok then error(value, 0) end
  if not answered then fail(result) end
  return result
end

-- One `tool_call` yield and its resume. A bound tool resumes as
-- `(ok, result)`; a local tool resumes as `(true, nil, handler, args)`,
-- and the handler runs here.
local function dispatch_tool(request)
  local ok, result, handler, args = yield(request)
  if not ok then fail(result) end
  if handler ~= nil then return run_local_tool(handler, args) end
  return result
end

-- Suspending dispatch of a tool. The first argument is the prompt-local
-- alias string or a Tool object; the alias-or-Tool polymorphism decodes
-- once, in the protocol parse. The driver resumes a bound tool's result by
-- the binding's declared output kind: a plain binding's text as a string,
-- a structured binding's JSON output as a table.
local function tools_call(alias_or_tool, args)
  return dispatch_tool({ op = "tool_call", alias = alias_or_tool, args = args })
end

-- The model-issued form of the same dispatch: `call_id` is the id the model
-- attached to its tool call, and `turn` the turn of the round that
-- requested it. The driver always resumes a bound tool with content (a
-- tool's own failure becomes untrusted failure text) and fires ToolResult
-- under the id and that turn, however far a local handler earlier in the
-- batch moved the counter. Only the test-only `tools.call_as_model` hook
-- calls it now, since the loop builds its `tool_call` requests in Rust;
-- authors never see it, and a hand-built yield including `call_id` or
-- `turn` is refused as a malformed request when its shape is wrong.
local function tools_call_as_model(call_id, alias_or_tool, args, turn)
  return dispatch_tool({
    op = "tool_call",
    alias = alias_or_tool,
    args = args,
    call_id = call_id,
    turn = turn,
  })
end

-- The loop's rules run in Rust, behind `loop_begin` and the step function
-- each call of it returns; this function only performs the action each
-- step returns. A Rust function called from Lua cannot yield, and cannot
-- call a Lua function that may yield, so the step hands back what to do
-- next instead of doing it: yield a request and pass every resume value
-- back; run a local tool's handler between the depth captures, or the
-- compactor, under the raw pcall and pass `ok` and the first result back;
-- raise a value at level 0; or return nil. The raw pcall keeps a Rust
-- callback's failure in mlua's own form for the step to judge.
local function drive(step, action, a, b)
  while true do
    if action == "yield" then
      action, a, b = step(yield(a))
    elseif action == "handler" then
      enter_local_handler()
      local ok, value = raw_pcall(a, b)
      leave_local_handler()
      action, a, b = step(ok, value)
    elseif action == "compactor" then
      local ok, failure = raw_pcall(a, b)
      action, a, b = step(ok, failure)
    elseif action == "raise" then
      error(a, 0)
    else
      return nil
    end
  end
end

-- models.loop(messages, compactor?) runs on the section's current model,
-- and a model handle's `loop`, as `h:loop(messages, compactor?)`, runs
-- every round on the handle's frozen binding. The Engine installs the
-- first as models.loop. Each entry names itself to `loop_begin`, so the
-- argument checks know the form without inspecting the arguments.
local function models_loop(...)
  return drive(loop_begin(false, ...))
end

local function handle_loop(...)
  return drive(loop_begin(true, ...))
end

-- store.*: every store operation is a leaf yield, answered by the driver
-- against the sync VFS uniformly for all backends - no inline fast path,
-- so interleaving behavior never depends on which backend serves the
-- mount. The Engine installs these onto the store table of section VMs and
-- the live H1 VM. `end` is a keyword, so the read bounds travel under
-- bracket keys.
local function store_request(store_op, fields)
  fields.op = "store"
  fields.store_op = store_op
  local ok, result = yield(fields)
  if not ok then fail(result) end
  return result
end

-- The block guard: the Engine runs every block coroutine through it so a
-- raised value is seen before mlua stringifies it. The raw `xpcall` is
-- yieldable, so the block's shim yields pass straight through; a return
-- passes through unchanged; a failure is stashed for the Engine (which reads
-- it back as the structured error when it is one of our tables) and
-- re-raised as the same value, so mlua's rendering, the retained-error
-- substitution, and the jump transfer marker all behave exactly as without
-- the guard. The stash happens in the message handler, which runs at the
-- raise point with the failing frames still on the stack, so the Engine can
-- record the real traceback there; by the time the guard re-raises, the
-- block's frames are unwound and mlua would see only the guard's own. The
-- guard deliberately bypasses the normalizing `pcall`: a Rust callback's
-- failure must reach the Engine as mlua's own error so the runtime-error
-- mapping keeps its source.
local function guard_handler(failure)
  stash_failure(failure)
  return failure
end

local function guard_outcome(ok, ...)
  if ok then return ... end
  error((...), 0)
end

local function guard(block)
  return guard_outcome(raw_xpcall(block, guard_handler))
end

local function store_write(path, contents)
  return store_request("write", { path = path, contents = contents })
end

local function store_append(path, contents)
  return store_request("append", { path = path, contents = contents })
end

local function store_read(path, start, finish)
  return store_request("read", { path = path, start = start, ["end"] = finish })
end

local function store_read_numbered(path, start, finish)
  return store_request("read_numbered", { path = path, start = start, ["end"] = finish })
end

local function store_str_replace(path, old, new)
  return store_request("str_replace", { path = path, old = old, new = new })
end

local function store_delete(path)
  return store_request("delete", { path = path })
end

local function store_glob(pattern)
  return store_request("glob", { pattern = pattern })
end

local function store_exists(path)
  return store_request("exists", { path = path })
end

-- The install passes the VM's `models` and `tools` tables.
models.infer = infer
tools.call = tools_call

return {
  call = call_section,
  -- The failure helpers, handed to the `tasks` and `fanout` chunks
  -- (`__impl_tasks.lua`, `__impl_fanout.lua`) so their shims raise the one
  -- error shape this prelude defines.
  helpers = { raise = raise, fail = fail, engine_type = engine_type },
  loop = models_loop,
  -- The fields a model handle reads, through the registry stash.
  handle_methods = { infer = handle_infer, loop = handle_loop },
  model_tool_call = tools_call_as_model,
  guard = guard,
  pcall = protected_call,
  xpcall = protected_xcall,
  store = {
    write = store_write,
    append = store_append,
    read = store_read,
    read_numbered = store_read_numbered,
    str_replace = store_str_replace,
    delete = store_delete,
    glob = store_glob,
    exists = store_exists,
  },
}
