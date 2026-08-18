---
name: store_triad
description: Exercise read_numbered (numbered) vs read (verbatim) vs untrusted-wrapped reads
promptforge: 1
---

# Store Triad

## Write

```lua
store.write("data.txt", "alpha\nbeta")
```

## Read

```lua
local numbered = store.read_numbered("data.txt")
local verbatim = store.read("data.txt")
local wrapped = untrusted(store.read("data.txt"))
var.numbered = numbered
var.verbatim = verbatim
var.has_tags = string.find(wrapped, "untrusted_input_") ~= nil
return numbered .. "|" .. verbatim
```
