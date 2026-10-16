---
name: hello
description: Say hello
runvil: 0
models:
  writer: {}
---

# Hello World

```lua
models.default("writer")
```

A minimal test prompt.

## Greet

Say "Hello, world!"

```lua
return models.infer(prose)
```
