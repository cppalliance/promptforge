---
name: real_text
description: Exercise one deterministic real-model text completion and epilog
promptforge: 1
max_tool_iterations: 1
---

# Real Text

```lua
models.always("writer", "A careful analysis model suited to structured reasoning and long-context review")
```

## Complete

Reply with exactly `PF_TEXT_OK` and no other text.

```lua
if type(reply) ~= "string" or reply == "" then
    error("real-text reply was empty")
end
log("real text epilog observed")
return "TEXT_EPILOG|" .. reply
```
