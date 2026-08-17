---
name: analyst_example
description: Demonstrate models.need and models.use for careful model resolution.
promptforge: 1
---

# Analyst Example

```lua
models.need("analyst", "A model suited for careful analysis", { thinking = false, temperature = 0, context = 40000 })
```

## Analyze

```lua
models.use("analyst")
```

Analyze the following input carefully and return a short factual summary with no preamble:

{{ args }}
