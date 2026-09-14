---
name: analyst_example
description: Demonstrate frontmatter model roles and models.use for careful model selection.
promptforge: 0
models:
  analyst:
    min_context: 40000
    description: A model suited for careful analysis
---

# Analyst Example

---

Demonstrates prompt-local model selection. The frontmatter `models:` key declares the `analyst` role, filled from the host's current model at prepare. A section that calls `models.use` runs every completion under that role's bound model; a section that omits `models.use` inherits the prompt-wide `models.default` model when one is declared.

## Analyze

```lua
models.use("analyst")
```

Analyze the following input carefully and return a short factual summary with no preamble:

{{ args }}

```lua
return models.infer(prose)
```
