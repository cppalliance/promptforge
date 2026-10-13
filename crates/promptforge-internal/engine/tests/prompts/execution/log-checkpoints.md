---
name: log_checkpoints
description: Record deterministic checkpoints across H1, prologue, and epilog phases
promptforge: 0
---

# Log Checkpoints

```lua
log("H1 ran")
```

## Prepare

```lua
log("prepare started")
store.write("state.txt", "prepared")
```

```lua
log("prepare finished")
```

## Finish

```lua
log("finish started")
return "logged"
```

This prose must not reach a model.

```lua
log("unreachable epilog")
return "late"
```
