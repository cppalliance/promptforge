---
name: fanout_epilog
description: Fanout invoked from the epilog with empty prose
runvil: 0
---

# Fanout Epilog

## Research

```lua
```

```lua
local replies = tasks.fanout("### Worker", prompt.list("### Items"))
return table.concat(replies, ",")
```

### Worker

```lua
return item .. "-" .. sys.index
```

### Items

- x
- y
