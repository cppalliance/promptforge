# Lua Scripting

A prompt is built from alternating Lua and prose blocks. Each section can contain any number of Lua blocks interleaved with prose segments. The last prose block in a section runs a full tool-call loop; earlier prose blocks run single-shot (one model round, then control continues to the next Lua block).

Preamble, prologue, and epilog are positions, not phases: the preamble is the H1 region, the prologue is a section's Lua before its first prose, and the epilog is Lua after the last prose. The behavior is emergent - an epilog runs simply because it is the next block.

A tool-call loop may end silently: when the model finishes with `finish_reason: "stop"` and an empty reply after completing at least one tool call, the loop accepts that as a clean exit and the section's `reply` is `""`. This supports "record everything via tools, output nothing" prompts. Any other empty reply - no prior tool calls, or a missing or non-`"stop"` finish reason - fails the run with an empty-model-reply error.

## The H1 Preamble

Lua blocks in the H1 region execute once in source order before any H2 section. The preamble declares tools and models, sets variables, and can short-circuit the entire run:

````markdown
# My Prompt

```lua
models.default("writer", "a capable writing model")
tools.need("search", "web search capability")
tools.always("search")
var.topic = "Rust async patterns"
```

## Write

Write an article about {{ var.topic }}.
````

Returning a scalar value (string, integer, number, or boolean) from H1 skips all H2 sections and becomes the run result.

## Shared Libraries

A `lua shared` fence in the H1 defines a reusable library compiled once and replayed into every section VM as its first chunk, with the full section environment already installed:

````markdown
```lua shared
function summarize(text)
    return "Summary: " .. text
end
```
````

The replay sees everything a later chunk sees - `args`, `sys`, `var`, `reply`, `store`, `log`, the `tools`/`models` tables, and the control globals - so top-level shared code may read `args` or write `store` files at load. Only the captured tool/model alias globals (the bare `search`, `analyst` handles) install after the replay, so a declared alias always wins over a same-named shared global. A scalar top-level return is discarded: the replay is a library load, not a result. `jump` is the one exclusion - calling it during the load fails the run with "jump is not available during shared library load".

## Section Environment

Each section VM provides these globals:

| Global | Purpose |
|--------|---------|
| `args` | Input string passed to the run |
| `sys` | Sealed read-only runtime metadata |
| `var` | Writable data bridge, persists across sections |
| `store` | Virtual filesystem |
| `tools` | Tool scope and call counts |
| `log` | Diagnostic checkpoint function |
| `reply` | Previous section's model answer |

The `sys` table includes `when`, `now`, `id`, `section_name`, `execution`, `section_count`, `model` (after first model interaction), and `reply_finish_reason` (after inference). It is sealed - writes raise errors and the metatable cannot be replaced. `sys.id` is the run-global execution-unit counter: H1 keeps id 0, and every section entry and every fanout arm takes the next value, so entering the same section twice yields two ids. A fanout arm also carries `sys.index`, its 1-based position within the current fanout (a nested fanout restarts at 1); `sys.index` is absent outside fanout, and reading it there raises.

`var` is the walk-local clipboard. Writes to it persist across sections on the same walk, H1 included, so H1 `var` writes are visible to every H2 section on the walk. `execute` and `fanout` start contained walks that clone the caller's `var` in and discard it out - child writes never reach the caller. `var` holds JSON data only: assigning a function, userdata, or a table containing one fails at the assigning line. Bare globals (`x = 42` without `local`) are section-local scratch instead, and prose reads them as `{{ x }}`.

## Template Substitution

Prose blocks support `{{ path }}` template substitutions. The sources are `args`, `reply`, `var`, `sys`, `item` (fanout arms only), and bare globals: an unknown first segment resolves as a section-local Lua global, with dotted paths indexing into its JSON form. Scalars render naturally, tables as JSON; a missing global, or one holding a function or userdata, is an error:

````markdown
## Research

```lua
var.query = "latest Rust async runtimes"
```

Search for {{ var.query }} and summarize the results for {{ args }}.
The previous section said: {{ reply }}
Current item: {{ item }}
Run id: {{ sys.id }}
````

Escape literal delimiters with backslash: `\{{` emits `{{`.

## Control Flow

`jump(target)` transfers control to another section by heading name, clearing conversation context. The current `reply` value is preserved across the jump, so the target section can reference it in prose (`{{ reply }}`) or Lua. Clear it explicitly with `reply = nil` before jumping when the target should not inherit the previous reply. `execute(target, input)` starts a contained chain at the target with a fresh VM and conversation, returning the chain's final reply:

````markdown
## Router

```lua
local result = execute("## Research", "find Rust crates for HTTP")
var.research = result
jump("## Synthesize")
```

## Research

Research the topic: {{ args }}

## Synthesize

Using this research: {{ var.research }}

Write a summary.
````

Both `jump` and `execute` address any section in the caller's visible set: its sibling sections at its own nesting level (for a top-level section, the other H2 sections) plus its direct children, disambiguated by heading level - `## Peer` matches only a sibling, `### Child` only a direct child. The parent, nieces and nephews, grandchildren, and the caller itself are not visible and resolve as not-found, with the error listing only the visible sections.

A jump to a child heading starts a child-level walk within the jumper's children: the walk begins at the target (which runs even when marked off-walk) and falls through to its following siblings under the same rules as the top-level walk. When the level exhausts, the parent walk resumes at the section after the jumper, and the sub-walk's last reply becomes the reply the next section sees. The rule recurses to deeper levels - a child can jump to its own children. A walk never descends on its own, so a section's children run only when addressed.

`execute()` runs a contained chain starting at its target: a walk with every normal rule - fall-through, off-walk skips, jumps, child chains - that never moves the outer walk. When the chain ends (its level exhausts or a `return` fires), the chain's final reply is the call's return value and the caller continues. A `return` ends only the chain it fires in; the top-level walk's return ends the run. Because a chain falls through like any walk, a multi-section subroutine is best expressed as a child walk (the children need no off-walk marker, since no walk descends on its own) or placed after the run-ending section.

Reply preservation across `jump()` enables routing patterns where one section's analysis determines the next section's context:

````markdown
## Analyze

Analyze this input for severity. End with exactly CRITICAL or NORMAL.

{{ args }}

```lua
if reply:find("CRITICAL") then
    jump("## Alert")
else
    jump("## Summary")
end
```

## Alert

The analysis found a critical issue:

{{ reply }}

Escalate this with recommended actions.
````

`execute()` nests up to 8 levels deep, and the count accumulates across `fanout` boundaries - each arm runs one level deeper than the section or arm that spawned it. A chain starts with `reply` set to nil - pass context through the `input` parameter instead. A `jump()` inside a chain moves within the chain, and a `return` inside a chain ends the chain, not the run. Sections are referenced by heading string.

`fanout(worker, collection)` maps the worker over any Lua table, resolved against the same visible set. The collection is always a table, never a section name - a non-table second parameter is an error that points at `list_from_section`. The array part (`1..#t`) iterates in order first, then the hash part in undefined order. An array member arrives as the arm's `item` unchanged - a string stays a string, a number a number, a table a table - while a hash member arrives as a pair table with `item.key` and `item.value`. Keys must be strings, numbers, or booleans; a function or userdata member is an error naming its index. Each arm result's `.item` carries the member value back, so the caller can correlate results with rich items. An empty collection is an error - no work is likely a bug. To fanout over a list section's pre-parsed items, pass `list_from_section("### List")` as the collection.

## Lua API Summary

| Function | Effect |
|----------|--------|
| `tools.need(alias, desc, override?)` | Resolve a tool by capability description; `override` sets the model-facing description |
| `tools.always(alias, override?)` | Make a resolved tool available in every section |
| `tools.add(alias, override?)` | Make a resolved tool available in this section; `tools.add({"a", "b"})` for bulk |
| `tools.add_local(alias, desc, params, handler)` | Declare a Lua-backed tool (H2 only) |
| `models.need(alias, desc, opts?)` | Resolve a model by capability description |
| `models.default(alias, desc, opts?)` | Declare and set the prompt-wide baseline model (H1) |
| `models.use(alias)` | Select a declared model for this section; returns its handle |
| `models.get(alias)` | Return a declared model's handle without changing the section model |
| `models.infer(prompt)` | One tool-free inference round on the section's current model |
| `handle:infer(prompt)` | One tool-free inference round on the handle's model |
| `store.*` | Virtual filesystem operations |
| `jump("## Section")` | Transfer control to a visible section (a sibling or a direct child); a child target starts a child-level walk |
| `execute("## Section", input?)` | Start a contained chain at a visible section (a sibling or a direct child); returns the chain's final reply |
| `fanout(worker, collection)` | Map a worker over a collection in parallel; array members arrive as `item`, hash members as pair tables |
| `list_from_section("## List")` | Return a list section's pre-parsed items as an array of strings |
| `log(msg)` | Emit a diagnostic to the observer |
| `untrusted(s)` | Wrap a string in the untrusted guard envelope |

Both infer forms share one shape: a single tool-free round on a fresh conversation that never sets `reply` and never touches `sys`. `models.infer(prompt)` uses the section's current model; `models.get(alias):infer(prompt)` uses any declared model. A Lua block that needs tools uses `execute` on a section.

## Local Tools

`tools.add_local(alias, description, params, handler)` declares a tool backed by a Lua function. When the model calls it during the tool loop, the handler runs synchronously in the section's VM instead of reaching an external service:

```lua
tools.add_local("extract_section", "Extract a range of lines from the paper", {
    name = {"string", "Section heading text"},
    start_line = {"integer", "1-based line number where the section begins"},
    end_line = {"integer", "1-based line number where the section ends"},
}, function(args)
    local lines = store.read_numbered("paper.md")
    return "extracted " .. args.name
end)
```

The alias must be unique within the section. It cannot reuse an alias declared by `tools.need` or `tools.always`, and a second `tools.add_local` call with the same alias is an error.

The params table maps each parameter name to either a bare type string or a `{type, description}` array. Supported types are `"string"`, `"integer"`, `"number"`, and `"boolean"`. All declared parameters are required. The engine converts the table into the JSON Schema the model sees.

The handler receives the arguments as a Lua table with the named fields and returns a string; Lua errors surface as tool-call failures. The handler shares the section's VM, so it can use `store`, `var`, and section globals, and it may call `execute()`, `fanout`, and the `infer` forms (`models.infer(prompt)`, `handle:infer(prompt)`). It cannot call `jump()` - `jump` is disabled for the duration of the call. Local tool output is trusted (no nonce envelope), since the prompt author wrote the handler. A local tool becomes visible to the model starting from the next prose block.

## Sandbox Constraints

The Lua sandbox provides only `string`, `table`, and `math` standard libraries. Dangerous globals (`load`, `dofile`, `require`, `print`, `rawget`, `rawset`, `collectgarbage`) are removed. A runaway Lua block is automatically aborted after exceeding the instruction budget (approximately 10 million instructions). Per-VM memory ceiling defaults to 64 MiB. The `log()` function accepts messages limited to 256 Unicode scalars with no newlines or control characters.

Tool and model aliases must match `[A-Za-z][A-Za-z0-9_-]{0,63}`.
