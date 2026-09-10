---
name: chat
description: The built-in Workshop chat agent on the unified runtime.
promptforge: 0
---

# Chat

The built-in chat agent: a transparent pass-through between the operator
and the selected model. The message list is an explicit Lua value retained
across turns; the model is re-read from the host snapshot every turn, so a
menu selection change takes effect on the next turn.

## Conversation

```lua
local history = messages.new()
while true do
    local text, available = user_input()
    if not available then
        return
    end
    history:user(text)
    local selected = ui().selected_model
    if selected then
        pcall(function() return models.loop(models.get(selected), history) end)
    end
end
```
