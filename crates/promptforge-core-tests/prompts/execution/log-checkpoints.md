---
name: log_checkpoints
description: Record deterministic checkpoints across shared, preamble, and epilog phases
promptforge: 1
---

# Log Checkpoints

```lua prompt
log("shared loaded")
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
