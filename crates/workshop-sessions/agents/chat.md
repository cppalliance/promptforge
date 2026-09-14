---
name: chat
description: The built-in Workshop chat agent on the unified runtime.
promptforge: 0
capabilities:
  - promptforge/web
tools:
  fetch: promptforge/web/fetch
  search: promptforge/web/search
models:
  chat: {}
---

# Chat

The built-in chat agent: a transparent pass-through between the operator
and the selected model. The message list is an explicit Lua value retained
across turns; the model is the dropdown's selection bound at run launch, so
a selection change takes effect on the next run.

```lua
models.default("chat")
tools.always("fetch")
tools.always("search")
```

## Conversation

```lua
local history = messages.new()
while true do
    local text, available = user_input()
    if not available then
        return
    end
    history:user(text)
    pcall(function() return models.loop(history) end)
end
```
