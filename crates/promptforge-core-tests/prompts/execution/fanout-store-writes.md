---
name: fanout_store_writes
description: Arms write to store with taskid
promptforge: 1
---

# Fanout Store Writes

## Research

```lua
local replies = fanout("### Worker", "### Topics")
local files = store.glob("arm-*.md")
return tostring(#files) .. ":" .. table.concat(replies, ",")
```

### Worker

```lua
store.write("arm-" .. sys.taskid .. ".md", item)
return item
```

Write to store.

### Topics

- alpha
- beta
