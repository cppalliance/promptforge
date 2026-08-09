---
name: analyst_example
description: Demonstrate models.need and models.use for a careful analysis binding.
promptforge: 1
---

# Analyst Example

```lua shared
models.need("analyst", "A model suited for careful analysis", { thinking = false, temperature = 0, context = 40000 })
```

Demonstrates prompt-local model binding. H1 `models.need` resolves against the host's gateway catalog. A section that calls `models.use` runs every completion under that binding's frozen invocation; a section that omits `models.use` inherits the prompt-wide `models.always` binding when one is declared.

## Analyze

```lua
models.use("analyst")
```

Analyze the following input carefully and return a short factual summary with no preamble:

{{ args }}
