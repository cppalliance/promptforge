---
name: fanout_basic
description: Two-item fanout with prologue-return arms
promptforge: 1
---

# Fanout Basic

## Research

```lua
local replies = fanout("### Worker", list_from_section("### Topics"))
return table.concat(replies, "\n")
```

### Worker

```lua
return item .. "-" .. sys.taskid
```

Do work.

### Topics

- alpha
- beta
