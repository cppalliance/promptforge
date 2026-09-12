---
name: fanout_store_writes
description: Arms write to store with sys.index
promptforge: 0
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
-- Arm-scoped writes: the pattern the claims model teaches. Each arm writes
-- only its own path, so no two live identities ever claim one path, and the
-- parent's post-join glob reads the merged state after every arm's claims
-- released at its end. (The old ready-*.md rendezvous - polling a sibling
-- arm's files while that arm is live - is exactly the cross-arm
-- read-while-written pattern the claims model rejects.)
store.write("arm-" .. sys.index .. ".md", item)
return item
```

Write to store.

### Topics

- alpha
- beta
