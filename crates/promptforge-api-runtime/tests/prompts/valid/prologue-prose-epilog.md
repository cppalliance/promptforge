---
name: phase_boundaries
description: Exercise an author-shaped prologue, prose, and epilog
promptforge: 0
max_tool_iterations: 3
---

# Phase Boundaries

Transform one model response.

## Transform

```lua
var.subject = args
```

Write about {{ var.subject }}.

```lua
return models.infer(prose)
```

## Fallback

This section has prose only.
