---
name: item_outside_fanout
description: Reading prose with an item substitution outside a fanout arm is an error
promptforge: 1
---

# Item Outside Fanout

## Main

This uses {{ item }} without being inside a fanout arm.

```lua
return prose
```
