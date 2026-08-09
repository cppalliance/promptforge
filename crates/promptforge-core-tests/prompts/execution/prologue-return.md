---
name: prologue_return
description: Return from a prologue before prose or epilog can run
promptforge: 1
---

# Prologue Return

## Stop Early

```lua
log("returning early")
return args
```

This prose must not reach a model.

```lua
log("unreachable epilog")
store.write("unreachable.txt", "epilog ran")
return "late"
```
