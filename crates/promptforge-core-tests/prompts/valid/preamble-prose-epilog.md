---
name: phase_boundaries
description: Exercise an author-shaped preamble, prose, and epilog
promptforge: 1
default_return: fallback
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
