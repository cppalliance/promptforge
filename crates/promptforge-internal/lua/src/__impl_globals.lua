-- The `_G` guard for a section VM, and the `setmetatable` and
-- `getmetatable` replacements that keep it in place.
--
-- The host runs this chunk first at VM construction, before hardening
-- strips the raw functions it captures and before any other chunk
-- captures `setmetatable` or `getmetatable`. The chunk arguments are
-- privileged captures, never globals: `globals` is the VM's globals table,
-- `state` is the host's slot table (`argv_frozen` and `argv` once a
-- section freezes `argv`, `prose` once a block installs its render, and
-- `author` for the metatable author code set on `_G`), and `refuse_argv()`
-- and `refuse_prose()` raise the host's assignment refusals. The chunk
-- returns the guard and the two replacements for the host to install.
local globals, state, refuse_argv, refuse_prose = ...

local base_setmetatable, base_getmetatable = setmetatable, getmetatable
local rawequal, rawget, rawset = rawequal, rawget, rawset
local error, next, pcall, select, type = error, next, pcall, select, type
local gsub = string.gsub

-- The guard is `_G`'s only metatable, and nothing hands it out: the base
-- `setmetatable` refuses to replace it, and the replacements below answer
-- for the author's metatable instead.
local guard = { __metatable = "_G is guarded" }

-- The guard's own fields. Every other field of the author's metatable is
-- copied onto the guard when the author sets it, so `_G` keeps the
-- author's other metamethods.
local own = { __index = true, __newindex = true, __metatable = true }

-- `argv` and `prose` never reach the author: `argv` serves the frozen value
-- once a section freezes it and is otherwise a plain global, so the H1
-- repair always lands in `_G`. Every other key goes to the author's
-- handler, read raw from the author's metatable on each access the way
-- Lua reads a metamethod, and tail-called so the handler's error levels
-- and yields behave as if it were the metamethod.
function guard.__index(t, key)
  if key == "argv" then
    return state.argv
  end
  if key == "prose" then
    local render = state.prose
    if render == nil then return nil end
    return render()
  end
  local author = state.author
  local handler = author and rawget(author, "__index")
  if handler == nil then return nil end
  if type(handler) == "function" then return handler(t, key) end
  return handler[key]
end

function guard.__newindex(t, key, value)
  if key == "argv" then
    if state.argv_frozen then return refuse_argv() end
    rawset(t, key, value)
    return
  end
  if key == "prose" then
    return refuse_prose()
  end
  local author = state.author
  local handler = author and rawget(author, "__newindex")
  if handler == nil then
    rawset(t, key, value)
  elseif type(handler) == "function" then
    return handler(t, key, value)
  else
    handler[key] = value
  end
end

local function forward(author)
  for key in next, guard do
    if not own[key] then guard[key] = nil end
  end
  if author == nil then return end
  for key, value in next, author do
    if not own[key] then guard[key] = value end
  end
end

-- A base function called through `pcall` cannot see the name it was
-- called by, so its argument error names it '?'; restore the name.
local function named(message, name)
  return (gsub(message, "^bad argument #(%d+) to '%?'", "bad argument #%1 to '" .. name .. "'", 1))
end

-- Re-raising at level 2 puts the caller's position on the message, where
-- the base function's own error would have put it.
local function replace_metatable(...)
  local target, metatable = ...
  if not rawequal(target, globals) then
    local ok, result = pcall(base_setmetatable, ...)
    if not ok then error(named(result, "setmetatable"), 2) end
    return result
  end
  if select("#", ...) < 2 or (metatable ~= nil and type(metatable) ~= "table") then
    local _, message = pcall(base_setmetatable, {}, select(2, ...))
    error(named(message, "setmetatable"), 2)
  end
  local author = state.author
  if author ~= nil and rawget(author, "__metatable") ~= nil then
    error("cannot change a protected metatable", 2)
  end
  forward(metatable)
  state.author = metatable
  return target
end

local function read_metatable(...)
  if rawequal((...), globals) then
    local author = state.author
    if author == nil then return nil end
    local protected = rawget(author, "__metatable")
    if protected ~= nil then return protected end
    return author
  end
  local ok, result = pcall(base_getmetatable, ...)
  if not ok then error(named(result, "getmetatable"), 2) end
  return result
end

return guard, replace_metatable, read_metatable
