---
name: fanout_store_writes
description: Arms write to store with sys.index
promptforge: 1
---

# Fanout Store Writes

## Research

```lua
local replies = fanout("### Worker", list_from_section("### Topics"))
local files = store.glob("arm-*.md")
return tostring(#files) .. ":" .. table.concat(replies, ",")
```

### Worker

```lua
-- Rendezvous: both arms must be live before either writes its reply path.
-- Each poll iteration yields through `execute`, giving the sibling arm its
-- I/O points: under the scheduler "concurrent" means interleaving at yield
-- points, not preemption. A sequential driver (arm 2 starting only after
-- arm 1 finishes) never reaches two ready files, and the loop spins until
-- the instruction budget trips.
store.write("ready-" .. sys.index .. ".md", "1")
while #store.glob("ready-*.md") < 2 do
  execute("## Yield")
end
store.write("arm-" .. sys.index .. ".md", item)
return item
```

Write to store.

### Topics

- alpha
- beta

## Yield

```lua
return "yielded"
```
