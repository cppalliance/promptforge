---
name: store_triad
description: Exercise read_lines (numbered) vs read (verbatim) vs inject (wrapped)
promptforge: 1
---

# Store Triad

## Write

```lua
store.write("data.txt", "alpha\nbeta")
```

## Read

```lua
local numbered = store.read_lines("data.txt")
local verbatim = store.read("data.txt")
local injected = store.inject("data.txt")
var.numbered = numbered
var.verbatim = verbatim
var.has_tags = string.find(injected, "untrusted_input_") ~= nil
return numbered .. "|" .. verbatim
```
