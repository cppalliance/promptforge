---
name: reply_nil_section_one
description: The removed reply register names no global, even in the first section
promptforge: 0
---

# Reply Nil Section One

## First

```lua
if reply ~= nil then
    error("the removed reply register must name no global")
end
return "section one done"
```
