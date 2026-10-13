-- PromptForge's `require` for a section VM.
--
-- The Engine installs this once per VM. The chunk arguments are Engine
-- captures, never globals: `resolve(heading)` asks the caller's resolver
-- for a library, from the position of the library loading now or, when
-- none is, of the section, and returns the library's identity, its
-- heading, and the library itself; `begin_load(library)` reports the
-- load's start, loads the library's blocks, marks the library as the one
-- loading, which makes `jump` refuse, and returns the blocks and their
-- prose templates in order; `end_load(finished)` takes that mark back and
-- reports how the load ended; `swap_prose(template)` installs a block's
-- own lazy `prose` over its template and returns the read it replaced;
-- `restore_prose(saved)` puts that read back; and `error_value` builds the
-- structured error table.
--
-- `require` is Lua rather than an Engine function, so the blocks run
-- inside the calling block's coroutine and a block's suspending call is an
-- ordinary yield of that block. Each load and each block's `prose` end
-- through to-be-closed variables rather than a protected call, so a raised
-- value passes through untouched and the block guard still records the
-- raise point.
local resolve, begin_load, end_load, swap_prose, restore_prose, error_value = ...

-- Captured at install, so author code that rebinds a global cannot move
-- them.
local error, setmetatable, concat = error, setmetatable, table.concat

-- Each library's saved value under its identity, or `LOADING` while its
-- blocks run. Keyed by identity, never by heading text, so one heading
-- resolved from two positions names two libraries.
local cache = {}
local LOADING = {}

-- The loads under way, outermost first, for the cycle message.
local loading = {}

local function raise(message)
  error(error_value("lua", { message = message }), 0)
end

-- The cycle a require of `identity` closes, from that library's own load
-- through the innermost: `require cycle: ## a -> ## b -> ## a`.
local function cycle(identity, heading)
  local chain = {}
  local inside = false
  for index = 1, #loading do
    local entry = loading[index]
    if entry.identity == identity then inside = true end
    if inside then chain[#chain + 1] = entry.heading end
  end
  chain[#chain + 1] = heading
  return "require cycle: " .. concat(chain, " -> ")
end

-- Ends one load on every exit, errors and cancellation included. A load
-- that never finished takes its marker back out of the cache, so the next
-- require runs the library again.
local closing = {
  __close = function(entry)
    if loading[#loading] == entry then loading[#loading] = nil end
    if not entry.finished then cache[entry.identity] = nil end
    if entry.begun then end_load(entry.finished) end
  end,
}

-- Hands back the `prose` read a library block's swap replaced, on every
-- exit, errors included, so the caller's memo survives the load.
local restoring = {
  __close = function(scope) restore_prose(scope.saved) end,
}

-- The first block that returns a value other than nil ends the library
-- with it; a library whose blocks all fall through hands back `true`.
-- Each block reads its own `prose` while it runs.
local function run(blocks, templates)
  for index = 1, #blocks do
    local scope <close> = setmetatable({ saved = swap_prose(templates[index]) }, restoring)
    local value = blocks[index]()
    if value ~= nil then return value end
  end
  return true
end

local function require(heading)
  local identity, library_heading, library = resolve(heading)
  local saved = cache[identity]
  if saved == LOADING then raise(cycle(identity, library_heading)) end
  if saved ~= nil then return saved end
  local entry <close> = setmetatable(
    { identity = identity, heading = library_heading, finished = false },
    closing
  )
  cache[identity] = LOADING
  loading[#loading + 1] = entry
  local blocks, templates = begin_load(library)
  entry.begun = true
  local value = run(blocks, templates)
  cache[identity] = value
  entry.finished = true
  return value
end

return require
