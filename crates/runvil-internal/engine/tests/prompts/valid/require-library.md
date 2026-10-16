---
name: require_library
description: Exercise a required library section and nested author prose
runvil: 0
---

# Require Library

The helper library is available to each section that requires it.

## Prepare

Normalize the supplied subject.

```lua
require '## helpers'
var.subject = normalize(args)
```

### Author note

This nested prose remains attached to Prepare.

## Finish

Return the normalized subject.

```lua
require '## helpers'
return normalize(var.subject)
```

## helpers

```lua
function normalize(value)
    return string.lower(value)
end
```
