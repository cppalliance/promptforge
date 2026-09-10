---
name: real_text
description: Exercise one deterministic real-model text completion and epilog
promptforge: 0
max_tool_iterations: 1
---

# Real Text

```lua
models.default("writer", "A careful analysis model suited to structured reasoning and long-context review")
```

## Complete

Reply with exactly `PF_TEXT_OK` and no other text.

```lua
local reply = models.infer(prose)
if type(reply) ~= "string" or reply == "" then
    error("real-text reply was empty")
end
log("real text epilog observed")
return "TEXT_EPILOG|" .. reply
```
