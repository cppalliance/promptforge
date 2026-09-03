---
name: phase_boundaries
description: Exercise an author-shaped prologue, prose, and epilog
promptforge: 1
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
return reply
```

## Fallback

This section has prose only.
