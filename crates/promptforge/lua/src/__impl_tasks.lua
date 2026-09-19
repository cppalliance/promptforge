-- The `tasks` namespace shims for a scheduler-mode section VM: spawn, the
-- waits, the non-blocking checks, the progress note, and cancel.
--
-- The host installs this after the coroutine prelude (`__impl_coro.lua`)
-- and installs the returned table as the `tasks` global. The chunk
-- arguments are privileged captures, never globals: `yield` is
-- coroutine.yield, `var_snapshot` is the host helper returning the hidden
-- `var` data table as a plain deep copy, and `helpers` is the prelude's
-- shared trio - `raise(kind, fields)` builds and raises the structured
-- error table, `fail(result)` raises an envelope's failure value, and
-- `host_type(value)` names a value's type as the protocol parse would.
--
-- A Task handle is a plain methodless table `{ task = id }` (A9): every
-- operation here is a namespace function that accepts the handle or the
-- bare id string, so a handle stored in `var` survives the serde boundary
-- unchanged, and a `when_all` result entry (which carries `task`) is
-- itself a handle.
local yield, var_snapshot, helpers = ...

local raise, fail, host_type = helpers.raise, helpers.fail, helpers.host_type

-- tasks.spawn(target, opts?): start a chain over `target` and return at
-- once with a Task handle. `opts.input` overrides the chain's args,
-- `opts.item` becomes its `item` global, `opts.index` its `sys.index`; the
-- caller's `var` seeds the chain. The origin is the shim's own fact, never
-- an argument: this surface is the author's.
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

-- Resolves a tasks.* argument to its bare id: a Task handle (any table
-- with a string `task` field) or the id string itself. Anything else is
-- the call's argument error.
local function task_id(value, call)
  if type(value) == "table" then value = value.task end
  if type(value) ~= "string" then
    raise("lua", { message = call .. " expects a Task handle or task id, got " .. host_type(value) })
  end
  return value
end

-- Resolves a wait's set argument to a non-empty sequence of bare ids.
local function task_set(set, call)
  if type(set) ~= "table" then
    raise("lua", { message = call .. " expects a set of tasks, got " .. host_type(set) })
  end
  local ids = {}
  for index, member in ipairs(set) do
    ids[index] = task_id(member, call)
  end
  if #ids == 0 then
    raise("lua", { message = call .. " requires at least one task" })
  end
  return ids
end

-- Resolves a wait's opts argument to its timeout in seconds, or nil when
-- no timeout was given. The domain check (non-negative, finite) is the
-- host's at the timer yield; the shape check is here so the message names
-- the call.
local function wait_timeout(opts, call)
  if opts == nil then return nil end
  if type(opts) ~= "table" then
    raise("lua", { message = call .. " opts must be a table, got " .. host_type(opts) })
  end
  local timeout = opts.timeout
  if timeout ~= nil and type(timeout) ~= "number" then
    raise("lua", { message = call .. " timeout must be a number, got " .. host_type(timeout) })
  end
  return timeout
end

-- Starts the internal timer behind a timed wait and returns its id. The
-- timer is an effect-backed task the caller owns and never sees: the wait
-- lists it beside the members and cancels it when a member wins. A
-- rejected timeout raises here, before any wait, with no timer started.
local function start_timer(seconds)
  local ok, result = yield({ op = "timer", seconds = seconds })
  if not ok then fail(result) end
  return result
end

-- Ends a timed wait's timer. Idempotent through the cancel arm: a timer
-- that already fired and was delivered is left as it is.
local function stop_timer(timer)
  local ok, result = yield({ op = "cancel", task = timer })
  if not ok then fail(result) end
end

-- One when_any round over `ids` with an optional live timer appended
-- after the members, so a finished member wins over a fired timer. Returns
-- the delivered id (or the timer's), ok, and result; a refused wait stops
-- the timer before it raises, so no timer outlives its wait.
local function wait_round(ids, timer)
  local set = ids
  if timer ~= nil then
    set = {}
    for index, id in ipairs(ids) do set[index] = id end
    set[#set + 1] = timer
  end
  local ok, task, task_ok, result = yield({ op = "when_any", tasks = set })
  if not ok then
    if timer ~= nil then stop_timer(timer) end
    fail(task)
  end
  return task, task_ok, result
end

-- tasks.when_any(set, opts?) -> Task, ok, result: park until the first
-- member of `set` ends (or return at once when one already has) and
-- return which one, whether it succeeded, and its final text or error
-- value. The one scheduler wait primitive; the error value is returned,
-- never raised, so the caller decides. A member the caller does not own
-- raises task_not_owned; a member already delivered raises task_consumed.
-- `opts.timeout` (seconds) returns nil when nothing finished in time; the
-- members keep running. When a member wins, the timer is cancelled.
local function tasks_when_any(set, opts)
  local ids = task_set(set, "tasks.when_any")
  local timeout = wait_timeout(opts, "tasks.when_any")
  local timer
  if timeout ~= nil then timer = start_timer(timeout) end
  local task, task_ok, result = wait_round(ids, timer)
  if timer ~= nil then
    if task == timer then return nil end
    stop_timer(timer)
  end
  return { task = task }, task_ok, result
end

-- tasks.when_all(set, opts?) -> results, timed_out: Lua over when_any.
-- Waits for every member and returns `{ task, ok, result }` per member in
-- input order; each entry is itself a Task handle. It never raises
-- because a member failed - the failed member's entry carries `ok = false`
-- and the error value - so no caller is forced into a cancel-or-leak
-- choice for the members still running. A member named twice is waited
-- on once and fills every position it was named at, so the result
-- sequence has no holes and `#results` is the input's length. With
-- `opts.timeout` (seconds), one timer spans every round: when it fires,
-- `timed_out` is true and the unfinished members' entries are absent;
-- when every member finishes first, the timer is cancelled.
local function tasks_when_all(set, opts)
  local ids = task_set(set, "tasks.when_all")
  local timeout = wait_timeout(opts, "tasks.when_all")
  local remaining, seen = {}, {}
  for _, id in ipairs(ids) do
    if not seen[id] then
      seen[id] = true
      remaining[#remaining + 1] = id
    end
  end
  local timer
  if timeout ~= nil then timer = start_timer(timeout) end
  local results, timed_out = {}, false
  while #remaining > 0 do
    local task, ok, result = wait_round(remaining, timer)
    if task == timer then
      timed_out = true
      break
    end
    for index, id in ipairs(ids) do
      if id == task then
        results[index] = { task = id, ok = ok, result = result }
      end
    end
    local rest = {}
    for _, id in ipairs(remaining) do
      if id ~= task then rest[#rest + 1] = id end
    end
    remaining = rest
  end
  if timer ~= nil and not timed_out then stop_timer(timer) end
  return results, timed_out
end

-- tasks.ready(task) -> boolean: whether the task has ended, without waiting.
local function tasks_ready(task)
  local ok, result = yield({ op = "ready", task = task_id(task, "tasks.ready") })
  if not ok then fail(result) end
  return result
end

-- tasks.status(task) -> table: the task's status. The caller may inspect a
-- task it owns or the task it runs inside (`sys.taskid`).
local function tasks_status(task)
  local ok, result = yield({ op = "status", task = task_id(task, "tasks.status") })
  if not ok then fail(result) end
  return result
end

-- tasks.pending(filter?) -> { Task, ... }: the caller's live tasks in spawn
-- order, narrowed to `filter.origin` (`author` or `model`) when given.
local function tasks_pending(filter)
  local origin
  if filter ~= nil then
    if type(filter) ~= "table" then
      raise("lua", { message = "tasks.pending filter must be a table, got " .. host_type(filter) })
    end
    origin = filter.origin
  end
  local ok, result = yield({ op = "pending", origin = origin })
  if not ok then fail(result) end
  local handles = {}
  for index, id in ipairs(result) do
    handles[index] = { task = id }
  end
  return handles
end

-- tasks.note(text): publish the caller's own task's latest progress note,
-- visible through tasks.status.
local function tasks_note(text)
  if type(text) ~= "string" then
    raise("lua", { message = "tasks.note text must be a string, got " .. host_type(text) })
  end
  local ok, result = yield({ op = "note", text = text })
  if not ok then fail(result) end
end

-- tasks.cancel(task): end a task the caller owns. Idempotent: cancelling
-- a task that already ended does nothing.
local function tasks_cancel(task)
  local ok, result = yield({ op = "cancel", task = task_id(task, "tasks.cancel") })
  if not ok then fail(result) end
end

return {
  spawn = tasks_spawn,
  when_any = tasks_when_any,
  when_all = tasks_when_all,
  ready = tasks_ready,
  status = tasks_status,
  pending = tasks_pending,
  note = tasks_note,
  cancel = tasks_cancel,
}
