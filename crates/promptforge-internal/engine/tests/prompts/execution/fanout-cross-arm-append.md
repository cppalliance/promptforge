---
name: fanout_cross_arm_append
description: Two arms append to one path
promptforge: 0
---

# Fanout Cross Arm Append

## Research

```lua
local replies = fanout("### Worker", {"alpha", "beta"})
return table.concat(replies, ",")
```

### Worker

```lua
-- The pcall proves the violation is not catchable from Lua: the
-- claims-model conflict never resumes into the arm, so this handler
-- never runs and the run terminates instead of returning "alpha,beta".
local ok, err = pcall(store.append, "evidence.md", item .. "\n")
store.write("caught-" .. sys.index .. ".txt", tostring(ok))
return item
```
