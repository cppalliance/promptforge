# Host state

This chapter teaches you what state the host exposes to your agent and how to read it. Two globals carry it: `ui`, a live snapshot of host state, and `sys`, a sealed table. Knowing the difference keeps you from trusting a stale read or poking a table that pushes back.

## Read the UI snapshot

````lua
local snapshot = ui()
log(snapshot.selected_model)
````

`ui()` returns a fresh host-state snapshot. Every call re-queries the host. There is no caching, so two calls in a row can legitimately give you two different answers.

Read a field the host has not set and you get nil: a JSON null field in the snapshot reads as nil, never as a placeholder.

The `ui` global exists only when the host supplies a provider. When the host supplies none, the global is absent entirely. Check for its presence before you rely on it.

## The sealed sys table

The `sys` global is installed sealed and empty in an agent run. Any field read raises an error that names the field. Access fields by string key only; any other key type raises. The table is read-only, and its seal cannot be replaced from your code. A present-but-null field reads as nil rather than as a placeholder, but an agent run has no such fields, so every read still raises. In an agent run there is nothing behind the seal, so treat `sys` as off limits.

