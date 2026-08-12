# Fanout

`fanout(worker, list)` maps a worker section over a list section's items in parallel. Each item is processed by its own isolated execution arm with a fresh Lua VM.

````markdown
## Process

```lua
local results = fanout("### Worker", "### URLs")
var.output = table.concat(results, "\n\n")
```

### Worker

Fetch and summarize: {{ item }}

### URLs

- https://example.com/page1
- https://example.com/page2
- https://example.com/page3
````

Worker and list sections are referenced by markdown heading address (level + name). A list-only section - one with only bullet items and no Lua blocks - serves as the fanout source.

## Arm Execution

Each arm receives the current item text as the `item` variable and a `sys.taskid` identifying its position. The arm can:

- Run a Lua prologue that short-circuits (enabling pure-Lua map operations)
- Substitute `{{ item }}` in prose
- Run the full model tool loop
- Execute an epilog for post-processing

Results are returned in list order (not finish order). Each result has `.text`, `.ok`, `.item`, and `.exhausted` fields. The result array supports `table.concat` since objects coerce via `__tostring`.

## Resilience

An exhausted arm (tool loop budget exceeded) soft-degrades into an incomplete stub rather than failing the entire fanout. A fatal error in any arm aborts all sibling arms, preventing wasted work. Cancellation propagates from the parent into each spawned arm cooperatively.

Default concurrency is 8 parallel arms, configurable via `RunLimits`.
