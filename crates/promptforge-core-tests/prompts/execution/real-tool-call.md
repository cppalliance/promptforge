---
name: real_tool_call
description: Exercise one aliased real-model tool call and continuation
promptforge: 1
max_tool_iterations: 2
---

# Real Tool Call

```lua
tools.need("ask_fixture", "Return one deterministic fixture value for one supplied string.")
tools.always("ask_fixture")
models.default("writer", "A careful analysis model suited to structured reasoning and long-context review")
```

## Call And Continue

Your first response must be a function call to `ask_fixture`, not text. Pass one argument object whose `value` is exactly `promptforge-probe`. The argument is not the tool result, and you cannot know the result before the function runs. Do not write `PF_TOOL_FINAL` in your first response. Only after receiving a tool-role result, reply with exactly `PF_TOOL_FINAL: ` followed by that complete result. Call no other function and call `ask_fixture` only once.

```lua
if type(reply) ~= "string" or reply == "" then
    error("real-tool-call final reply was empty")
end
log("real tool epilog observed")
return "TOOL_EPILOG|" .. reply
```
