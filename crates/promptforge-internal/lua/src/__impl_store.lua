-- The `store` table's function values for a section VM: each one forwards
-- to its direct Engine closure until the store yield shims install, then to
-- the matching shim (`__impl_coro.lua`), so a function the shared library
-- captured at load time (`local write = store.write`) follows the load
-- phase exactly as a fresh `store.write` lookup does.
--
-- The Engine builds one dispatcher per operation with the returned
-- `dispatcher(name, direct)` and keeps the returned `phase` table; the
-- shim install sets `phase.shims` to the prelude's store shim table. Both
-- stay out of author reach: `phase` is an upvalue here and a registry
-- value on the Rust side.
local phase = {}

local function dispatcher(name, direct)
  return function(...)
    local shims = phase.shims
    if shims then
      return shims[name](...)
    end
    return direct(...)
  end
end

return dispatcher, phase
