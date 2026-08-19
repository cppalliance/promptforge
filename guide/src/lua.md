# Lua Scripting

A prompt is built from alternating Lua and prose blocks. Each section can contain any number of Lua blocks interleaved with prose segments. The last prose block in a section runs a full tool-call loop; earlier prose blocks run single-shot (one model round, then control continues to the next Lua block).

A tool-call loop may end silently: when the model finishes with `finish_reason: "stop"` and an empty reply after completing at least one tool call, the loop accepts that as a clean exit and the section's `reply` is `""`. This supports "record everything via tools, output nothing" prompts. Any other empty reply - no prior tool calls, or a missing or non-`"stop"` finish reason - fails the run with an empty-model-reply error.

## The H1 Phase

Lua blocks in the H1 region execute once in source order before any H2 section. The H1 phase declares tools and models, sets variables, and can short-circuit the entire run:

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
| `tasks` | Section handles for control flow |

The `sys` table includes `when`, `now`, `id`, `section_name`, `execution`, `section_count`, `model` (after first model interaction), and `reply_finish_reason` (after inference). It is sealed - writes raise errors and the metatable cannot be replaced.

## Template Substitution

Prose blocks support `{{ path }}` template substitutions with five namespaces:

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

`jump(target)` transfers control to another section by heading name, clearing conversation context. The current `reply` value is preserved across the jump, so the target section can reference it in prose (`{{ reply }}`) or Lua. Clear it explicitly with `reply = nil` before jumping when the target should not inherit the previous reply. `execute(target, input)` runs a section as a subroutine with a fresh VM and conversation, returning that section's reply:

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

`execute()` nests up to 8 levels deep. A subroutine starts with `reply` set to nil - pass context through the `input` parameter instead. `jump()` inside an `execute()` subroutine is rejected with a clear error. Sections can be referenced by heading string or by Section objects from the `tasks` table.

## Lua API Summary

| Function | Effect |
|----------|--------|
| `tools.need(alias, desc)` | Resolve a tool by capability description |
| `tools.always(alias...)` | Make resolved tools available in every section |
| `tools.add(alias...)` | Make resolved tools available in this section |
| `tools.add_local(alias, desc, params, handler)` | Declare a Lua-backed tool (H2 only) |
| `models.need(alias, desc, opts?)` | Resolve a model by capability description |
| `models.default(alias, desc, opts?)` | Declare and set the prompt-wide baseline model (H1) |
| `models.use(alias)` | Select a declared model for this section; returns its handle |
| `models.get(alias)` | Return a declared model's handle without changing the section model |
| `models.infer(prompt)` | One tool-free inference round on the section's current model |
| `handle:infer(prompt)` | Tool-loop inference on a specific model handle |
| `store.*` | Virtual filesystem operations |
| `jump("## Section")` | Transfer control to a visible section (a sibling or a direct child); a child target starts a child-level walk |
| `execute("## Section", input?)` | Run a visible section (a sibling or a direct child) as a subroutine |
| `fanout(worker, list)` | Map a worker over a list section in parallel |
| `list_from_section("## List")` | Return a list section's pre-parsed items as an array of strings |
| `log(msg)` | Emit a diagnostic to the observer |
| `untrusted(s)` | Wrap a string in the untrusted guard envelope |

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

The params table maps each parameter name to either a bare type string or a `{type, description}` array. Supported types are `"string"`, `"integer"`, `"number"`, and `"boolean"`. All declared parameters are required. The engine converts the table into the JSON Schema the model sees.

The handler receives the arguments as a Lua table with the named fields and returns a string; Lua errors surface as tool-call failures. The handler shares the section's VM, so it can use `store`, `var`, and section globals, and it may call `execute()`, `fanout`, and `model:infer`. It cannot call `jump()` - `jump` is disabled for the duration of the call. Local tool output is trusted (no nonce envelope), since the prompt author wrote the handler. A local tool becomes visible to the model starting from the next prose block or `model:infer` call.

## Sandbox Constraints

The Lua sandbox provides only `string`, `table`, and `math` standard libraries. Dangerous globals (`load`, `dofile`, `require`, `print`, `rawget`, `rawset`, `collectgarbage`) are removed. A runaway Lua block is automatically aborted after exceeding the instruction budget (approximately 10 million instructions). Per-VM memory ceiling defaults to 64 MiB. The `log()` function accepts messages limited to 256 Unicode scalars with no newlines or control characters.

Tool and model aliases must match `[A-Za-z][A-Za-z0-9_-]{0,63}`.
