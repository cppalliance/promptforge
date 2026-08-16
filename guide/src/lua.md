# Lua Scripting

A prompt is built from alternating Lua and prose blocks. Each section can contain any number of Lua blocks interleaved with prose segments. The last prose block in a section runs a full tool-call loop; earlier prose blocks run single-shot (one model round, then control continues to the next Lua block).

## The H1 Phase

Lua blocks in the H1 region execute once in source order before any H2 section. The H1 phase declares tools and models, sets variables, and can short-circuit the entire run:

````markdown
# My Prompt

```lua
models.always("writer", "a capable writing model")
tools.need("search", "web search capability")
tools.always("search")
var.topic = "Rust async patterns"
```

## Write

Write an article about {{ var.topic }}.
````

Returning a scalar value (string, integer, number, or boolean) from H1 skips all H2 sections and becomes the run result.

## Shared Libraries

A `lua shared` fence in the H1 defines a reusable library compiled once and loaded into every section VM:

````markdown
```lua shared
function summarize(text)
    return "Summary: " .. text
end
```
````

Shared functions resolve host globals (`store`, `log`, `args`) at call time, not load time - so a shared function can reference `store` even though it doesn't exist when the library loads.

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

The `sys` table includes `when`, `now`, `id`, `section_name`, `execution`, `section_count`, `model` (after scope closure), and `reply_finish_reason` (after inference). It is sealed - writes raise errors and the metatable cannot be replaced.

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

`execute()` nests up to 8 levels deep. `jump()` inside an `execute()` subroutine is rejected with a clear error. Sections can be referenced by heading string or by Section objects from the `tasks` table.

## Sandbox Constraints

The Lua sandbox provides only `string`, `table`, and `math` standard libraries. Dangerous globals (`load`, `dofile`, `require`, `print`, `rawget`, `rawset`, `collectgarbage`) are removed. A runaway Lua block is automatically aborted after exceeding the instruction budget (approximately 10 million instructions). Per-VM memory ceiling defaults to 64 MiB. The `log()` function accepts messages limited to 256 Unicode scalars with no newlines or control characters.

Tool and model aliases must match `[A-Za-z][A-Za-z0-9_-]{0,63}`.
