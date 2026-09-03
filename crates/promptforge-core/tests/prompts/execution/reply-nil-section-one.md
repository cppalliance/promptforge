---
name: reply_nil_section_one
description: First section sees nil reply, returns a value
promptforge: 1
---

# Reply Nil Section One

## First

```lua
if reply ~= nil then
    error("reply must be nil in section 1")
end
return "section one done"
```
