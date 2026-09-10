-- Coroutine-protocol shim prelude for a scheduler-mode section VM.
--
-- The host installs this after the host tables exist and before the shared
-- library replays. The chunk arguments are privileged captures, never
-- globals: `yield` is coroutine.yield (the coroutine global is stripped
-- after install, so author code cannot yield directly), `var_snapshot` is
-- the host helper returning the hidden `var` data table as a plain deep
-- copy, and `models`/`tools` are the section's namespace tables, passed in
-- so the chunk never reads a global.
local yield, var_snapshot, models, tools = ...

-- The (ok, result) envelope: level 0 suppresses the position prefix, so a
-- shim-raised error carries exactly the host's message.
--
-- models.infer(handle?, prompt): an optional leading model handle runs the
-- round on the handle's frozen binding; without one the driver resolves the
-- section's current model. Invocation is namespace-only (A9): handles are
-- plain inspectable userdata with no colon methods.
local function infer(...)
  local handle, prompt
  if select('#', ...) > 2 then
    error("models.infer takes (handle?, prompt)", 0)
  elseif select('#', ...) == 2 then
    handle, prompt = ...
  else
    prompt = ...
  end
  local ok, result = yield({ op = "infer", prompt = prompt, handle = handle })
  if not ok then error(result, 0) end
  return result
end

local function call_section(target, input)
  local ok, result = yield({
    op = "call",
    target = target,
    input = input,
    var = var_snapshot(),
  })
  if not ok then error(result, 0) end
  return result
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
  if not ok then error(result, 0) end
  return result
end

-- Suspending dispatch of a bound tool. The first argument is the
-- prompt-local alias string or a Tool object; the alias-or-Tool
-- polymorphism decodes once, in the protocol parse. The driver resumes the
-- result by the binding's declared output kind: a plain binding's text as a
-- string, a structured binding's JSON output as a table.
local function tools_call(alias_or_tool, args)
  local ok, result = yield({ op = "tool_call", alias = alias_or_tool, args = args })
  if not ok then error(result, 0) end
  return result
end

-- One stateless tool-capable model round. The host installs this as
-- models.chat in agent VMs only; a section VM never sees it. Both
-- arguments pass through unvalidated: the protocol parse owns the whole
-- messages/opts validation, so every argument error surfaces at this call
-- site (pcall-able) with no second validator anywhere.
local function chat(messages, opts)
  local ok, result = yield({ op = "chat", messages = messages, opts = opts })
  if not ok then error(result, 0) end
  return result
end

-- models.loop(handle?, messages, compactor?): the Rust-backed model-tool
-- loop over an author-owned message list. The host installs this as
-- models.loop in section VMs only; an agent VM never sees it. The leading
-- handle is optional: a userdata first argument selects the handle's frozen
-- binding, anything else is the messages argument (a wrong handle type is
-- the protocol parse's call error, exactly as for models.infer). The loop
-- appends every assistant message and correlated tool result to the list
-- and returns nil.
local function models_loop(...)
  local handle, messages, compactor
  if type((...)) == 'userdata' then
    if select('#', ...) > 3 then
      error("models.loop takes (handle?, messages, compactor?)", 0)
    end
    handle, messages, compactor = ...
  else
    if select('#', ...) > 2 then
      error("models.loop takes (handle?, messages, compactor?)", 0)
    end
    messages, compactor = ...
  end
  local ok, result = yield({
    op = "loop",
    handle = handle,
    messages = messages,
    compactor = compactor,
  })
  if not ok then error(result, 0) end
  return result
end

-- user_input(): direct operator input through the run's input broker. The
-- host installs this as a global in section VMs only; an agent VM never
-- sees it. The resume is (ok, text, available): on success the call
-- returns the text plus the availability flag, so the broker's fixed
-- fallback sentence cannot be spoofed by identical human text; on failure
-- the call raises the host's message at the call site.
local function user_input(...)
  if select('#', ...) > 0 then
    error("user_input takes no arguments", 0)
  end
  local ok, text, available = yield({ op = "user_input" })
  if not ok then error(text, 0) end
  return text, available
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
  fanout = fanout_collection,
  chat = chat,
  infer = infer,
  loop = models_loop,
  user_input = user_input,
}
