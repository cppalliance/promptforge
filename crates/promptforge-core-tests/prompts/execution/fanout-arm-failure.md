---
name: fanout_arm_failure
description: Arm error propagates to invoker
promptforge: 1
---

# Fanout Arm Failure

## Research

```lua
local replies = fanout("### Worker", list_from_section("### Topics"))
return table.concat(replies, "\n")
```

### Worker

```lua
error("arm deliberately failed")
```

Work.

### Topics

- alpha
- beta
