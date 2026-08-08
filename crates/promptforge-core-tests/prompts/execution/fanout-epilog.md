---
name: fanout_epilog
description: Fanout invoked from the epilog with empty prose
promptforge: 1
---

# Fanout Epilog

## Research

```lua
```

```lua
local replies = fanout("### Worker", "### Items")
return table.concat(replies, ",")
```

### Worker

```lua
return item .. "-" .. sys.taskid
```

### Items

- x
- y
