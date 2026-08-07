---
name: dev_verification
description: Dev-loop verification prompt exercising one model reply and an epilog
promptforge: 1
max_tool_iterations: 1
---

# Dev Verification

This prompt verifies the dev command end to end against the real dev model: a
preamble checkpoint, one substituted model turn, and an epilog that must see
the reply.

## Reply

```lua
log("dev verification preamble")
var.subject = args
```

Reply with one short sentence about {{ var.subject }}, then on its own final line write exactly `PF_DEV_OK` and nothing else on that line.

```lua
if type(reply) ~= "string" or reply == "" then
    error("dev verification reply was empty")
end
log("dev verification epilog observed")
return "DEV_EPILOG|" .. reply
```
