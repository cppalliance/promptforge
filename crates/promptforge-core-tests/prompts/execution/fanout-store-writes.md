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
-- Sequential fanout never reaches two ready files and hangs until the test times out.
store.write("ready-" .. sys.index .. ".md", "1")
while #store.glob("ready-*.md") < 2 do
end
store.write("arm-" .. sys.index .. ".md", item)
return item
```

Write to store.

### Topics

- alpha
- beta
