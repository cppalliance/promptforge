---
name: list_h3_with_lua
description: A list H3 with Lua has no parsed items, so fanout fails at runtime
promptforge: 1
---

# List H3 With Lua

## Parent

```lua
local replies = fanout("### Worker", "### Items")
return table.concat(replies, "\n")
```

### Worker

```lua
return item
```

Work.

### Items

```lua
var.x = 1
```

- alpha
- beta
