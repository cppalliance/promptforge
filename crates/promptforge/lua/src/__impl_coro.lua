-- Coroutine-protocol shim prelude for a scheduler-mode section VM.
--
-- The host installs this after the host tables exist and before the shared
-- library replays. The chunk arguments are privileged captures, never
-- globals: `yield` is coroutine.yield (the coroutine global is stripped
-- after install, so author code cannot yield directly), `var_snapshot` is
-- the host helper returning the hidden `var` data table as a plain deep
-- copy, `models`/`tools`/`compactors` are the section's namespace tables,
-- passed in so the chunk never reads a global, `max_tool_iterations` is
-- the run's resolved round cap for `models.loop`, `error_value` builds the
-- structured error table (`{ kind, message, ... }` under the host's shared
-- metatable, whose `__tostring` is `message`), `stash_failure` records a
-- block's raised value for the host before the guard re-raises it, and
-- `normalize_failure` turns a Rust callback's raised failure (mlua's
-- opaque userdata) into the error table, passing every other value
-- through unchanged.
local yield, var_snapshot, models, tools, compactors, max_tool_iterations,
  error_value, stash_failure, normalize_failure = ...

-- The base library's pcall and xpcall, captured before the replacements
-- below are installed over the globals: the block guard needs the raw
-- failure value so the host's runtime-error mapping keeps its source.
local raw_pcall, raw_xpcall = pcall, xpcall

-- math.type, captured at install for the same reason: the shim's type
-- names must not move when author code rebinds `math`.
local math_type = math.type

-- The host's name for a value's type, as the protocol parse reports it:
-- Lua folds integers and floats into "number", while the host names an
-- integer "integer" and a float "number", so a shim-raised argument error
-- reads exactly as the parse-raised one for the same value.
local function host_type(value)
  if math_type(value) == "integer" then return "integer" end
  return type(value)
end

-- Every failure that reaches author code is one error table: `tostring`
-- gives exactly the message, and a caller that branches reads `kind` and
-- the kind's fields. Level 0 suppresses the position prefix (a table never
-- gets one, but a string fallback would), so a shim-raised error carries
-- exactly the host's message.
local function raise(kind, fields)
  error(error_value(kind, fields), 0)
end

-- The (ok, result) envelope's failure path. The host renders its typed
-- error as the table already; a bare string (a hand-built envelope) is
-- normalized to a `lua`-kind table so the shape holds without exception.
local function fail(result)
  if type(result) == "table" then error(result, 0) end
  raise("lua", { message = tostring(result) })
end

-- pcall and xpcall, replacing the base library's over the globals so a
-- host callback that fails directly from Rust (`tools.add`, `models.get`,
-- a `sys` or `var` guard) reaches author code as the same error table a
-- shim raise does, instead of mlua's opaque userdata that `err.kind`
-- cannot index. Only a Rust-raised failure is rewritten; a string, an
-- author's own table, and an error table already built pass through
-- untouched. The raw pcall is yieldable, and so is this Lua frame, so a
-- shim yield inside the protected function still suspends the block.
local function pcall_outcome(ok, ...)
  if ok then return true, ... end
  return false, normalize_failure((...))
end

local function protected_call(f, ...)
  return pcall_outcome(raw_pcall(f, ...))
end

-- The message handler sees the normalized failure; a non-function handler
-- is left to the raw xpcall so its own argument error is unchanged.
local function protected_xcall(f, handler, ...)
  if type(handler) ~= "function" then
    return raw_xpcall(f, handler, ...)
  end
  return raw_xpcall(f, function(failure)
    return handler(normalize_failure(failure))
  end, ...)
end

-- models.infer(handle?, prompt): an optional leading model handle runs the
-- round on the handle's frozen binding; without one the driver resolves the
-- section's current model. Invocation is namespace-only (A9): handles are
-- plain inspectable userdata with no colon methods.
local function infer(...)
  local handle, prompt
  if select('#', ...) > 2 then
    raise("lua", { message = "models.infer takes (handle?, prompt)" })
  elseif select('#', ...) == 2 then
    handle, prompt = ...
  else
    prompt = ...
  end
  local ok, result = yield({ op = "infer", prompt = prompt, handle = handle })
  if not ok then fail(result) end
  return result
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

-- tasks.spawn(target, opts?): start a chain over `target` and return at
-- once with a Task handle. `opts.input` overrides the chain's args,
-- `opts.item` becomes its `item` global, `opts.index` its `sys.index`; the
-- caller's `var` seeds the chain. The handle is a plain methodless table
-- `{ task = id }` (A9): every `tasks.*` operation is a namespace function
-- that accepts the table or the bare id, so a handle stored in `var`
-- survives the serde boundary unchanged. The origin is the shim's own
-- fact, never an argument: this surface is the author's.
local function tasks_spawn(target, opts)
  if opts == nil then
    opts = {}
  elseif type(opts) ~= "table" then
    raise("lua", { message = "tasks.spawn opts must be a table, got " .. host_type(opts) })
  end
  local ok, result = yield({
    op = "spawn",
    target = target,
    input = opts.input,
    item = opts.item,
    index = opts.index,
    var = var_snapshot(),
    origin = "author",
  })
  if not ok then fail(result) end
  return { task = result }
end

-- The collection passes through unconverted; the driver runs the
-- member-wise conversion at the protocol boundary.
local function fanout_collection(worker, collection)
  local ok, result = yield({
    op = "fanout",
    worker = worker,
    collection = collection,
    var = var_snapshot(),
  })
  if not ok then fail(result) end
  return result
end

-- Suspending dispatch of a bound tool. The first argument is the
-- prompt-local alias string or a Tool object; the alias-or-Tool
-- polymorphism decodes once, in the protocol parse. The driver resumes the
-- result by the binding's declared output kind: a plain binding's text as a
-- string, a structured binding's JSON output as a table.
local function tools_call(alias_or_tool, args)
  local ok, result = yield({ op = "tool_call", alias = alias_or_tool, args = args })
  if not ok then fail(result) end
  return result
end

-- The model-issued form of the same dispatch: `call_id` is the id the model
-- attached to its tool call. The driver always resumes it with content (a
-- tool's own failure becomes untrusted failure text) and fires ToolResult
-- under the id. Shim-internal: the loop shim calls it per requested tool
-- call; authors never see it, and a hand-built yield carrying `call_id` is
-- refused as a malformed request when its shape is wrong.
local function tools_call_as_model(call_id, alias_or_tool, args)
  local ok, result = yield({
    op = "tool_call",
    alias = alias_or_tool,
    args = args,
    call_id = call_id,
  })
  if not ok then fail(result) end
  return result
end

-- One stateless tool-capable model round. The host installs this as
-- models.chat in agent VMs only; a section VM never sees it. Both
-- arguments pass through unvalidated: the protocol parse owns the whole
-- messages/opts validation, so every argument error surfaces at this call
-- site (pcall-able) with no second validator anywhere.
local function chat(messages, opts)
  local ok, result = yield({ op = "chat", messages = messages, opts = opts })
  if not ok then fail(result) end
  return result
end

-- Drains pending model-task notices into the author's list ahead of a
-- round. No task exists yet, so nothing drains; the hook is the seam the
-- tasks namespace fills in.
local function drain_task_notices(messages)
end

-- Appends one record to the author's list. The list is a plain array (a
-- messages.new() list keeps its builders behind __index, never as
-- fields), so the append is an ordinary sequence store.
local function append_record(messages, record)
  messages[#messages + 1] = record
end

-- The message the exit rules raise for an empty round when the round
-- carried no phrase of its own.
local EMPTY_MODEL_REPLY = "empty model reply"

-- Invokes the selected compactor on an overflow round with the reason tag.
-- The shipped policy raises typed context exhaustion from Rust; the raise
-- is normalized into the structured error table before re-raising, so the
-- kind reaches an author pcall and the host alike. A compactor that
-- returns instead of raising is the deferred replacement shape, which the
-- active surface refuses.
local function compact(compactor, reason)
  local ok, failure = raw_pcall(compactor, reason)
  if ok then
    raise("lua", {
      message = "the selected compactor returned without raising: replacement compactors are "
        .. "deferred; compactors.fail is the only shipped policy",
    })
  end
  error(normalize_failure(failure), 0)
end

-- models.loop(handle?, messages, compactor?): the model-tool loop over an
-- author-owned message list, driven here over `chat` and `tool_call`
-- yields so every network wait inside it is an ordinary suspension. The
-- host installs this as models.loop in section VMs only; an agent VM
-- never sees it. The leading handle is optional: a userdata first argument
-- selects the handle's frozen binding, anything else is the messages
-- argument (a wrong handle type is the protocol parse's call error,
-- exactly as for models.infer). The compactor defaults to compactors.fail.
--
-- Per round: drain pending task notices, yield one `chat` over the list;
-- on an overflow round invoke the compactor; on tool calls yield one
-- `tool_call` per call under its call id, buffer every result, then append
-- the assistant tool-call record and one tool record per result, so the
-- list never shows a half-answered batch; on a reply append it and return
-- nil; on an empty reply with `finish_reason == "stop"` after at least one
-- answered tool call append an empty assistant record and return nil (the
-- model's clean exit); on any other empty reply raise empty_model_reply.
-- Past the round cap raise tool_loop_exhausted. The shim emits no events:
-- the scheduler reports each round as it applies the round's answer.
local function models_loop(...)
  local handle, messages, compactor
  if type((...)) == 'userdata' then
    if select('#', ...) > 3 then
      raise("lua", { message = "models.loop takes (handle?, messages, compactor?)" })
    end
    handle, messages, compactor = ...
  else
    if select('#', ...) > 2 then
      raise("lua", { message = "models.loop takes (handle?, messages, compactor?)" })
    end
    messages, compactor = ...
  end
  if compactor == nil then
    compactor = compactors.fail
  elseif type(compactor) ~= "function" then
    raise("lua", { message = "compactor must be a function, got " .. host_type(compactor) })
  end
  -- Answered dispatches: any call that received a result record, a tool's
  -- own failure included, counts toward the clean-exit rule.
  local answered = 0
  for _ = 1, max_tool_iterations do
    drain_task_notices(messages)
    local ok, round = yield({ op = "chat", messages = messages, handle = handle })
    if not ok then fail(round) end
    if round.overflow then
      compact(compactor, round.overflow_reason)
    end
    local calls = round.tool_calls
    if calls then
      local results = {}
      for index, call in ipairs(calls) do
        results[index] = tools_call_as_model(call.id, call.name, call.arguments)
      end
      local record_calls = {}
      for index, call in ipairs(calls) do
        record_calls[index] = { id = call.id, name = call.name, arguments = call.arguments }
      end
      append_record(messages, { role = "assistant", content = "", tool_calls = record_calls })
      for index, call in ipairs(calls) do
        append_record(messages, { role = "tool", content = results[index], tool_call_id = call.id })
      end
      answered = answered + #calls
    elseif round.reply then
      append_record(messages, { role = "assistant", content = round.reply })
      return nil
    elseif round.finish_reason == "stop" and answered > 0 then
      append_record(messages, { role = "assistant", content = "" })
      return nil
    else
      raise("empty_model_reply", {
        message = round.empty_detail or EMPTY_MODEL_REPLY,
        finish_reason = round.finish_reason,
      })
    end
  end
  raise("tool_loop_exhausted", { message = "tool-call loop did not converge" })
end

-- user_input(): direct operator input through the run's input broker. The
-- host installs this as a global in section VMs only; an agent VM never
-- sees it. The resume is (ok, text, available): on success the call
-- returns the text plus the availability flag, so the broker's fixed
-- fallback sentence cannot be spoofed by identical human text; on failure
-- the call raises the host's message at the call site.
local function user_input(...)
  if select('#', ...) > 0 then
    raise("lua", { message = "user_input takes no arguments" })
  end
  local ok, text, available = yield({ op = "user_input" })
  if not ok then fail(text) end
  return text, available
end

-- store.*: every store operation is a leaf yield, answered by the driver
-- against the sync VFS uniformly for all backends - no inline fast path,
-- so interleaving behavior never depends on which backend serves the
-- mount. The host installs these onto the store table of section VMs and
-- the live H1 VM only; an agent VM's store table keeps its direct
-- closures. `end` is a keyword, so the read bounds travel under bracket
-- keys.
local function store_request(store_op, fields)
  fields.op = "store"
  fields.store_op = store_op
  local ok, result = yield(fields)
  if not ok then fail(result) end
  return result
end

-- The block guard: the host runs every block coroutine through it so a
-- raised value is seen before mlua stringifies it. The raw `xpcall` is
-- yieldable, so the block's shim yields pass straight through; a return
-- passes through unchanged; a failure is stashed for the host (which reads
-- it back as the structured error when it is one of our tables) and
-- re-raised as the same value, so mlua's rendering, the retained-error
-- substitution, and the jump transfer marker all behave exactly as without
-- the guard. The stash happens in the message handler, which runs at the
-- raise point with the failing frames still on the stack, so the host can
-- record the real traceback there; by the time the guard re-raises, the
-- block's frames are unwound and mlua would see only the guard's own. The
-- guard deliberately bypasses the normalizing `pcall`: a Rust callback's
-- failure must reach the host as mlua's own error so the runtime-error
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

-- The section install passes the section's namespace tables; the live H1
-- base install passes nil for both (H1's live models table exists only per
-- block, given the shim by the host's per-step wrap) and takes `infer` from
-- the return.
if models then
  models.infer = infer
end
if tools then
  tools.call = tools_call
end

return {
  call = call_section,
  tasks = {
    spawn = tasks_spawn,
  },
  fanout = fanout_collection,
  chat = chat,
  infer = infer,
  loop = models_loop,
  model_tool_call = tools_call_as_model,
  user_input = user_input,
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
