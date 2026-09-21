---
name: store_fallthrough
description: Write store state in one section and read it in the next
promptforge: 0
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
return store.read("handoff.txt")
```
