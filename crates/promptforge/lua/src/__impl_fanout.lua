-- The `fanout` shim for a scheduler-mode section VM: Lua over the task
-- protocol (`spawn`, `when_any`, `cancel`), so every wait inside a fanout
-- is an ordinary yield and the scheduler keeps no fanout state of its own.
--
-- The host installs this after the coroutine prelude (`__impl_coro.lua`)
-- and installs the returned function as the `fanout` global. The chunk
-- arguments are privileged captures, never globals: `yield` is
-- coroutine.yield, `var_snapshot` is the host helper returning the hidden
-- `var` data table as a plain deep copy, `helpers` is the prelude's shared
-- trio (`raise(kind, fields)` builds and raises the structured error
-- table, `fail(result)` raises an envelope's failure value, and
-- `host_type(value)` names a value's type as the protocol parse would),
-- `max_fanout_concurrency` is the run's cap on live arms,
-- `collection_members(collection)` enumerates a collection as `(members)`
-- or `(nil, message)` - the array part in order, then the hash part as
-- `{ key, value }` pairs sorted by key - and `render_item(item)` renders a
-- member as `{{ item }}` would.
local yield, var_snapshot, helpers, max_fanout_concurrency,
  collection_members, render_item = ...

local raise, fail = helpers.raise, helpers.fail

-- setmetatable, captured at install so the shim's behavior does not move
-- when author code rebinds the base globals.
local setmetatable = setmetatable

-- One fanout arm's result: a frozen, methodless object (A9) whose fields
-- are `text`, `ok`, `item`, and `exhausted`, with `tostring` giving the
-- text so a `table.concat` over the results keeps working. Writes raise:
-- the result is the arm's record, not the author's scratch space. The
-- seal has three parts: the fields live in a hidden `__index` table the
-- author cannot reach, `__newindex` refuses every assignment, and
-- `__metatable` hands `getmetatable` a decoy (containing only `__tostring`,
-- so the hardened `table.concat` still recognizes the result as
-- renderable) and makes `setmetatable` refuse to replace the guard. The
-- one remaining bypass would be `rawset`, which the VM's hardening pass
-- removes from the globals before any author code runs.
local function fanout_result(item, text, ok, exhausted)
  local fields = { text = text, ok = ok, item = item, exhausted = exhausted }
  local function render() return text end
  return setmetatable({}, {
    __index = fields,
    __newindex = function() error("fanout results are read-only", 2) end,
    __tostring = render,
    __metatable = { __tostring = render },
  })
end

-- The incomplete stub an exhausted arm's slot receives: one stuck arm must
-- not kill its siblings' evidence, so its text says what happened.
local function exhausted_stub(item)
  return "## " .. render_item(item) .. "\n\nUNKNOWN\n\n(section incomplete: tool loop exhausted)"
end

-- fanout(worker, collection): run `worker` once per collection member as
-- a task chain and return the results in collection order. The members
-- enumerate (array part in order, then the hash part as `{ key, value }`
-- pairs sorted by key), an empty collection raises before any spawn, up to
-- `max_fanout_concurrency` arms are live at once (one spawned to refill
-- the window on every completion), each arm is spawned with the member as
-- its `item`, its 1-based position as `sys.index`, and the `fanout` mark
-- (so the spawn arm's depth-cap refusal is named after `fanout`, the name
-- the cap always had on this path, and re-raises here as the retained
-- typed error), and `when_any` over the live set delivers the arms as
-- they end. A `tool_loop_exhausted` arm
-- becomes the incomplete stub and the fanout continues; any other arm
-- failure cancels the live arms and re-raises. No arm outlives the call:
-- every arm is delivered or cancelled before the function returns or
-- raises.
local function fanout(worker, collection)
  local members, message = collection_members(collection)
  if not members then
    raise("lua", { message = message })
  end
  local count = #members
  if count == 0 then
    raise("lua", { message = "fanout over an empty collection: no work is likely a bug" })
  end
  -- One snapshot serves every arm: the caller is suspended inside this
  -- call, so its `var` cannot move between spawns.
  local var = var_snapshot()
  local results = {}
  -- The live arms: `slot_of[id]` is the arm's collection index, `live`
  -- the ids in spawn order (the `when_any` set, so an earlier arm wins a
  -- tie).
  local slot_of, live = {}, {}
  local next_index = 1

  -- Ends every live arm. Best effort on an error path: a refused cancel
  -- would only mask the failure already being raised.
  local function cancel_live()
    for _, id in ipairs(live) do
      yield({ op = "cancel", task = id })
    end
    live, slot_of = {}, {}
  end

  local function spawn_next()
    local index = next_index
    next_index = index + 1
    local ok, result = yield({
      op = "spawn",
      target = worker,
      item = members[index],
      index = index,
      var = var,
      origin = "author",
      fanout = true,
    })
    if not ok then
      cancel_live()
      fail(result)
    end
    slot_of[result] = index
    live[#live + 1] = result
  end

  while next_index <= count and #live < max_fanout_concurrency do
    spawn_next()
  end
  while #live > 0 do
    local ok, task, arm_ok, result = yield({ op = "when_any", tasks = live })
    if not ok then
      cancel_live()
      fail(task)
    end
    local index = slot_of[task]
    slot_of[task] = nil
    local rest = {}
    for _, id in ipairs(live) do
      if id ~= task then rest[#rest + 1] = id end
    end
    live = rest
    local item = members[index]
    if arm_ok then
      results[index] = fanout_result(item, result, true, false)
    elseif type(result) == "table" and result.kind == "tool_loop_exhausted" then
      results[index] = fanout_result(item, exhausted_stub(item), false, true)
    else
      cancel_live()
      fail(result)
    end
    if next_index <= count then spawn_next() end
  end
  return results
end

return fanout
