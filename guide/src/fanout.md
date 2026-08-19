# Fanout

`fanout(worker, collection)` maps a worker section over a collection in parallel. Each member is processed by its own isolated execution arm with a fresh Lua VM.

````markdown
## Process

```lua
local results = fanout("### Worker", list_from_section("### URLs"))
var.output = table.concat(results, "\n\n")
```

### Worker

Fetch and summarize: {{ item }}

### URLs

- https://example.com/page1
- https://example.com/page2
- https://example.com/page3
````

The worker is referenced by markdown heading address (level + name) and resolves against the caller's visible sections - its siblings plus its direct children. The second parameter is always a collection, never a section name: any Lua table works, and `list_from_section("### List")` feeds a list section's pre-parsed bullet or numbered items straight in.

The array part (`1..#t`) iterates in order first, then the hash part in undefined order. An array member arrives as the arm's `item` unchanged - a string stays a string, a number a number, a table a table. A hash member arrives as a pair table: `item.key` and `item.value`. Keys must be strings, numbers, or booleans; a function or userdata member is an error naming its index. An empty collection returns an empty result table.

## Arm Execution

Each arm receives the current member as the `item` variable and a `sys.taskid` identifying its 1-based position in the collection. The arm can:

- Run a Lua prologue that short-circuits (enabling pure-Lua map operations)
- Substitute `{{ item }}` in prose (strings verbatim, numbers and booleans in their natural string form, tables as compact JSON)
- Run the full model tool loop
- Execute an epilog for post-processing
- Call `execute`, `fanout`, and `list_from_section`, resolved against the worker's visible sections (the set the worker was resolved from, minus the worker, plus its children), and transfer control with `jump` - the arm's remaining blocks are skipped and the arm's text becomes the jumped-to walk's reply. Recursion depth accumulates across the fanout boundary: each arm runs one `execute` level deeper than its caller, so the 8-level cap bounds mixed `execute`/`fanout` nesting uniformly

Results are returned in collection order (array part first, then the hash part), not finish order. Each result has `.text`, `.ok`, `.item`, and `.exhausted` fields; `.item` carries the member value back - a pair table for hash members - so the caller can correlate results with rich items. The result array supports `table.concat` since objects coerce via `__tostring`.

## Resilience

An exhausted arm (tool loop budget exceeded) soft-degrades into an incomplete stub rather than failing the entire fanout. A fatal error in any arm aborts all sibling arms, preventing wasted work. Cancellation propagates from the parent into each spawned arm cooperatively.

Default concurrency is 8 parallel arms, configurable via `RunLimits`.
