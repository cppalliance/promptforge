---
name: store_fallthrough
description: Carry explicit store state across isolated fall-through sections
promptforge: 1
---

# Store Fall-through

## Write

```lua
log("writing state")
store.write("handoff.txt", args)
```

## Read

```lua
log("reading state")
return store.read_lines("handoff.txt")
```
