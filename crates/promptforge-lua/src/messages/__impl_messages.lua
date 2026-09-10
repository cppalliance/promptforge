-- Pure-Lua message-list builders for the `messages` namespace.
--
-- `messages.new()` returns a normal numerically indexed table. The
-- chainable methods live behind the table's metatable __index, never as
-- direct fields, so the list itself stays a plain array of message records:
-- the host's serde conversion, prose substitution, and the chat protocol's
-- validation consume the records exactly as if the author had written the
-- array by hand. The builders validate nothing; the protocol parse owns the
-- whole message contract, so a malformed record fails at the models.chat
-- call site, pcall-able, exactly like a hand-written array.

local function new()
  local list = {}

  local function push(record)
    list[#list + 1] = record
    return list
  end

  -- Colon-call methods: the list itself is the first argument and the
  -- return value, so `list:user("a"):assistant("b")` chains.
  local methods = {}

  function methods.system(_, content)
    return push({ role = "system", content = content })
  end

  function methods.user(_, content)
    return push({ role = "user", content = content })
  end

  -- `tool_calls` is optional: an absent argument leaves the field unset, so
  -- the record reads back exactly like a hand-written text-only turn.
  function methods.assistant(_, content, tool_calls)
    local record = { role = "assistant", content = content }
    if tool_calls ~= nil then
      record.tool_calls = tool_calls
    end
    return push(record)
  end

  function methods.tool(_, content, tool_call_id)
    return push({ role = "tool", content = content, tool_call_id = tool_call_id })
  end

  -- Appends one author-built record unchanged.
  function methods.append(_, record)
    return push(record)
  end

  return setmetatable(list, { __index = methods })
end

return new
