---
name: greet
description: Greet the named input using a Lua-computed value
version: 1
---

# Greet

Computes a greeting from the input in Lua, substitutes it into the prose, and
has the model echo it.

## Main

```lua
var.greeting = "Hello, " .. args .. "!"
```

Repeat exactly, with no extra words: {{ var.greeting }}
