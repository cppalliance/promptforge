---
name: fanout_epilog
description: Fanout invoked from the epilog with empty prose
promptforge: 0
---

# Fanout Epilog

## Research

```lua
```

```lua
local replies = fanout("### Worker", list_from_section("### Items"))
return table.concat(replies, ",")
```

### Worker

```lua
return item .. "-" .. sys.index
```

### Items

- x
- y
