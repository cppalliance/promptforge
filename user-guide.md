# PromptForge User Guide

A progressive tutorial for writing PromptForge prompts. Each section adds one concept with a working example. By the end you will have written a multi-section pipeline prompt with tools, models, store, fanout, and programmatic control flow.

You need PromptForge installed, a gateway running, and `PROMPTFORGE_GATEWAY_KEY` set. You know Markdown and have seen Lua.

---

## 1. What Is a Prompt File

```markdown
---
name: hello
description: Say hello to the user.
promptforge: 1
---

# Hello

## Main

Say hello and tell the user something interesting about the number 42.
```

A prompt file is Markdown with three parts:

- **Frontmatter** - YAML between `---` fences. `name` and `description` are required at parse time. `promptforge: 1` is required when you run the prompt (engine major must be `1`).
- **H1 heading** - exactly one. This is the prompt's title and starts the live H1 program.
- **H2 sections** - executable units. They run top to bottom.

Ordinary Lua and prose between the H1 and the first H2 form the live H1 program. They run once before any H2 section. A file may also place one exact `lua shared` library fence there for functions that section VMs need.

Save this as `hello.md` and run it:

```
promptforge run hello.md
```

The prose under `## Main` goes to the model. The model's response is the prompt's output.

A prompt file is Markdown that declares its engine version, has one title, and runs its sections in order.

## 2. The Model Turn

```markdown
---
name: summarize
description: Summarize the input.
promptforge: 1
---

# Summarize

## Main

Summarize the following text in one sentence:

{{ args }}
```

Run it:

```
promptforge run summarize.md "The quick brown fox jumped over the lazy dog."
```

Here is the mental model: prose becomes a user message. The model responds. That response is the prompt's output.

The `{{ args }}` placeholder is replaced with the input string before the model sees it. Substitution is covered in section 8.

When no Lua is present and no tools are scoped, one round trip happens: prose in, text out. That text is the run's result.

A model turn sends prose as a user message and returns the model's response as output.

## 3. Your First Lua Block (the Prologue)

````markdown
---
name: greet
description: Greet someone by name.
promptforge: 1
---

# Greet

## Main

```lua
models.use("writer")
var.greeting = "Hello, " .. args .. "!"
```

Repeat exactly, with no extra words: {{ var.greeting }}
````

The fenced `lua` block before the prose is the **prologue**. It runs before the model turn. Inside it you can:

- Select a model with `models.use`
- Set variables on `var` for use in prose substitution
- Read `args` (the input string)
- Call `tools.add` to scope tools for this section

The prologue is an exact, unindented ` ```lua ` opening and ` ``` ` closing. Indented fences, longer backtick runs, or different capitalization are treated as ordinary prose.

`models.use("writer")` selects a model resolved in live H1 (covered next). Without H1 resolving models, this would fail. For now, note the pattern: the prologue sets up what the model turn needs.

The prologue runs Lua before the model sees the prose.

## 4. Live H1 and the Shared Library

````markdown
---
name: fetcher
description: Fetch and summarize a URL.
promptforge: 1
---

# Fetcher

```lua
models.always("writer",
    "A model suited for careful analysis and summarization",
    { thinking = false, temperature = 0, context = 32768 })
tools.need("fetch", "Fetch a web page and return its main content as markdown.")
```

A tool that fetches URLs and summarizes their content.

```lua
var.subject = args
```

```lua shared
function summarize_request(subject)
    return "Summarize " .. subject
end
```

## Main

```lua
tools.add("fetch")
```

Fetch {{ args }} and summarize its content in three bullet points.
````

PromptForge parses the file and runs it directly. H1 is a live, single-pass program whose ordinary Lua and prose blocks run once in source order before any H2 section:

```
[lua] [prose] [lua] [prose] ... [lua]
```

H1 Lua has `args`, `sys`, `var`, and `store`. `tools.need`, `models.need`, and `models.always` resolve immediately when execution reaches the call, so conditional needs are natural and skipped branches resolve nothing. Model objects can call `model:infer()` in H1, H1 prose uses the model selected by `models.always`, and the final H1 `var` value seeds the sections. Non-final H1 prose makes one model round, dispatches any tool calls from that round, and then continues to the next Lua block. The final H1 prose runs the full tool loop until text is produced. A text response becomes `reply`; a tool-only non-final response leaves `reply` unchanged.

The exact `lua shared` fence is a separate shared library chunk. It may appear anywhere between the H1 and the first section, but it is not part of the live H1 block sequence. Use it for functions that sections need, not for live capability resolution.

Each section, `execute()` subroutine, and fanout arm gets a fresh VM. Promptforge loads the `lua shared` chunk into that VM before injecting `args`, `sys`, `var`, `store`, `reply`, `log`, or capability bindings. A top-level host call in `lua shared`, such as `store.write(...)`, `tools.need(...)`, `model:infer(...)`, or `log(...)`, therefore fails while the library loads. Function definitions may refer to host globals: Lua resolves those names when the function is called later, once Promptforge has installed both the host and the captured Tool and Model objects directly from Rust.

A prompt may contain at most one `lua shared` fence, and it must be in H1. A second `lua shared` fence, or one under an H2 or deeper heading, is a parse error. The opening marker must be the exact, unindented text ` ```lua shared ` with an exact ` ``` ` closing marker.

A plain H1 `lua` fence is not a shared library. It is parsed and executed as an ordinary live H1 Lua block, even when it is the only H1 block. Use the explicit `shared` tag for code that must be loaded into section VMs.

`models.always` does three things in one live H1 call: declares a model need, resolves it against the gateway catalog, and sets it as the default for H1 prose and all sections. Sections that call `models.use` override this default. The combined form takes an alias, a capability description, and an optional table of constraints. Both forms return a Model object.

`tools.need` declares a semantic capability need and returns a Tool object. The alias is your local name. The description tells the tool picker what you need. Declaring a need does not expose the tool to the model. That requires `tools.add` in a section Lua block before tool scope closes.

Live H1 also accepts `models.need` (declare without selecting as default) and `tools.always` (expose a tool in every section).

**Globals vs locals.** Names in live H1 are not shared by Lua heap identity with section VMs. Rust captures resolved Tool and Model bindings and the serialized `var` table for sections. Put reusable pure functions in `lua shared`.

```lua
search = tools.need("search", "Search the web and return a list of results.")
var.query = args
```

The `search` binding and `var.query` are available to sections through the captured run state. A plain H1 local is not.

The explicit `lua shared` library is for reusable definitions. Live H1 performs host calls and capability resolution.

## 5. The Epilog

````markdown
---
name: uppercase
description: Return the model's reply in uppercase.
promptforge: 1
---

# Uppercase

```lua
models.always("writer",
    "A general-purpose model",
    { thinking = false, temperature = 0, context = 8192 })
```

## Main

Tell me a fun fact about {{ args }}.

```lua
return string.upper(reply)
```
````

The fenced `lua` block after the prose is the **epilog**. It runs after the model finishes responding. The model's response text is available as the global `reply`.

The epilog shares the same VM as earlier Lua in that section - variables set in the prologue are still accessible. You can:

- Inspect and transform `reply`
- Write to the store
- Check `tools.calls` counts
- Return a value to end the run

A scalar `return` from **any** section Lua block ends the entire run with that value. If you do not return, execution falls through to the next section (or the next block in an alternating section).

A classic section's three phases are: prologue (setup), prose (model turn), epilog (post-processing). All three are optional. Alternating blocks generalize this pattern (section 13).

The epilog runs after the model responds, in the same VM as the prologue, and can inspect or transform the reply.

## 6. Multiple Sections

````markdown
---
name: two_step
description: Research then report.
promptforge: 1
---

# Two Step

```lua
models.always("writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 16384 })
```

## Research

List five key facts about {{ args }}.

## Report

Using only the facts below, write a one-paragraph summary.

Facts:
{{ reply }}
````

Sections execute in file order. Each section gets:

- A fresh Lua VM (no Lua state carries over from the previous section)
- A fresh model conversation (no message history carries over)
- The previous section's `reply` as a global and as `{{ reply }}` in prose

Two channels bridge sections: `reply` (the previous section's model output) and the `store` (covered next). Everything else resets.

`{{ reply }}` in section 1 is nil and using it there is a hard error. In section 2 and beyond, it contains the previous section's model response.

Multiple sections let you build pipelines where each step reads the previous step's output through `reply`.

## 7. The Store

````markdown
---
name: store_demo
description: Demonstrate store read and write.
promptforge: 1
---

# Store Demo

```lua
models.always("writer",
    "A general-purpose model",
    { thinking = false, temperature = 0, context = 8192 })
```

## Collect

```lua
store.write("notes.md", "# Notes on " .. args .. "\n")
```

Write three facts about {{ args }}. Number them.

```lua
store.append("notes.md", "\n\n## Model Output\n\n" .. reply)
```

## Summarize

```lua
var.notes = store.inject("notes.md")
```

Summarize the following notes in one sentence:

{{ var.notes }}

```lua
store.write("summary.md", reply)
return reply
```
````

The store is a run-scoped virtual filesystem. Files exist only for the duration of the run and are shared by live H1 and all sections.

| Operation | What it does |
|---|---|
| `store.write(path, text)` | Create or overwrite a file |
| `store.append(path, text)` | Append to a file |
| `store.read(path)` | Return verbatim contents |
| `store.read_lines(path)` | Return numbered lines (`1\| ...`) |
| `store.inject(path)` | Return contents wrapped in an untrusted envelope |
| `store.str_replace(path, old, new)` | Replace exact text in a file |
| `store.delete(path)` | Remove a file |
| `store.glob(pattern)` | List matching paths |
| `store.exists(path)` | Check if a file exists |

Use `store.inject` when the contents will be sent to the model - it wraps the text in a nonce-framed envelope that marks it as untrusted data. Use `store.read` when you need the raw text in Lua.

The store provides run-scoped virtual files that persist across sections and fanout arms.

## 8. Template Substitution

````markdown
---
name: template_demo
description: Show all substitution forms.
promptforge: 1
---

# Template Demo

```lua
models.always("writer",
    "A general-purpose model",
    { thinking = false, temperature = 0, context = 8192 })
```

## Main

```lua
var.topic = args
var.count = 3
var.tags = { "history", "science" }
```

Date: {{ sys.when }}
Topic: {{ var.topic }}
Requested count: {{ var.count }}
Tags: {{ var.tags }}

Write {{ var.count }} facts about {{ var.topic }}.
````

Namespaces available in prose substitution:

| Placeholder | Source |
|---|---|
| `{{ args }}` | The raw input string |
| `{{ reply }}` | Previous section's model reply (nil in section 1) |
| `{{ var.x }}` | Values set in the section's `var` table |
| `{{ sys.when }}` | Run launch timestamp |
| `{{ sys.now }}` | Current section start time |
| `{{ sys.id }}` | 1-based section index |
| `{{ sys.section_name }}` | Current section heading name |
| `{{ sys.execution }}` | Run execution id |
| `{{ sys.section_count }}` | Number of top-level H2 sections |
| `{{ sys.model }}` | Bound model catalog id (after scope close) |
| `{{ sys.reply_finish_reason }}` | Last inference finish reason (after a model turn) |
| `{{ sys.taskid }}` | 1-based arm index (fanout only) |
| `{{ item }}` | Current fanout item text (fanout only) |

Substitution rules:

- Scalars render as strings
- Lua tables render as JSON
- A missing path is a hard error
- Substitution applies only to prose, never to Lua source
- `{{ reply }}` in section 1, or `{{ item }}` outside a fanout arm, is a hard error

Template substitution resolves `{{ path }}` placeholders in prose before the model sees the text.

## 9. Tools (Search and Fetch)

````markdown
---
name: researcher
description: Research a topic using web search.
promptforge: 1
---

# Researcher

```lua
models.always("writer",
    "A careful research model",
    { thinking = false, temperature = 0, context = 32768 })
tools.need("search", "Search the web and return a list of results.")
tools.need("fetch", "Fetch a URL and return its main content as markdown.")
```

## Main

```lua
tools.add("search", "fetch")
```

Research {{ args }}. Search for relevant sources, fetch the most
promising result, and write a three-paragraph summary based on
what you find. Cite your source URLs.
````

Tools let the model reach outside the prompt during a section. Two tools ship built in:

- **search** - searches the web and returns trimmed results (title, URL, description). Proxied through the gateway, which holds the Brave API key.
- **fetch** - fetches a URL and returns its main content as markdown. Runs in-process, no credential needed.

The tool loop works like this:

1. Prose and scoped tool schemas go to the model
2. If the model responds with a tool call, the executor dispatches it
3. The tool result is appended to the conversation
4. The model is called again
5. This repeats until the model responds with text (not a tool call)

The loop is capped at `max_tool_iterations` (default 24) per section (or per `model:infer` call) to prevent runaway loops. Set it in frontmatter to change the cap.

`tools.add` accepts alias strings, Tool objects, and arrays of either. Only tools added in Lua before the section's tool scope closes (plus any `tools.always` from live H1) are visible to the model. Scope closes on the first prose block.

Tools give the model the ability to search and fetch during a section's model turn.

## 10. The Tool Object

````markdown
---
name: tool_inspect
description: Inspect tool objects.
promptforge: 1
---

# Tool Inspect

```lua
models.always("writer",
    "A general-purpose model",
    { thinking = false, temperature = 0, context = 8192 })
search = tools.need("search",
    "Search the web and return a list of results.")
fetch = tools.need("fetch",
    "Fetch a URL and return its main content as markdown.")
```

## Main

```lua
log("search tool: " .. search.name)
log("fetch description: " .. fetch.description)

search.description = "Find web pages about " .. args
tools.add(search, fetch)
```

Search for information about {{ args }} and summarize what you find.
````

`tools.need` returns a Tool object. Call it in live H1. Rust captures the resolved alias and installs the corresponding Tool object for sections.

| Property | Type | Description |
|---|---|---|
| `.name` | string | The alias you declared |
| `.description` | string | Model-facing description (mutable) |
| `.parameters` | table | Parameter schema (currently an empty object until registry enrichment) |
| `.wire_name` | string | Stable identity name used on the wire |
| `.untrusted` | boolean | Whether results are marked untrusted (currently always `false` on handles from `tools.need`) |

The `.description` property is mutable. Changing it before `tools.add` overrides what the model sees for that add.

`tools.add` accepts Tool objects, strings, and arrays of either:

```lua
tools.add(search)                    -- single object
tools.add("search", "fetch")        -- strings
tools.add({search, fetch})          -- array of objects
tools.add(search, "fetch")          -- mixed
```

Tool objects are first-class values you can inspect, customize, and pass to `tools.add`.

## 11. The Model Object

````markdown
---
name: model_inspect
description: Inspect model objects.
promptforge: 1
---

# Model Inspect

```lua
writer = models.need("writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 32768 })
fast = models.need("fast",
    "A fast general model",
    { thinking = false, temperature = 0, context = 8192 })
models.always("writer")
```

## Main

```lua
log("writer: " .. writer.name)
log("model_id: " .. writer.model_id)
log("context: " .. tostring(writer.context))
log("fast model: " .. fast.name)
```

Tell me one fact about {{ args }}.
````

`models.need` and both forms of `models.always` return a Model object. Use globals when section Lua must see the handle.

| Property | Type | Description |
|---|---|---|
| `.name` | string | The alias you declared |
| `.model_id` | string | Resolved catalog model id |
| `.description` | string | Capability description |
| `.context` | number | Context window size in tokens |
| `.thinking` | boolean or nil | Frozen thinking preference (`true` / `false` / unset) |
| `.temperature` | number or nil | Frozen temperature, if set |
| `.max_tokens` | number or nil | Frozen max tokens, if set |
| `.dialect` | string | Tool dialect name |

All properties are read-only. The Model object is frozen - its parameters are locked when live H1 resolves it and cannot change during execution.

You can declare multiple models with `models.need` and select between them per section with `models.use`. `models.always` sets the prompt-wide default.

Model objects let you inspect resolved model properties and select between multiple resolved models.

## 12. Explicit Inference with model:infer()

````markdown
---
name: gated_research
description: Search first, then fetch, then summarize.
promptforge: 1
---

# Gated Research

```lua
writer = models.always("writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 32768 })
tools.need("search", "Search the web and return a list of results.")
tools.need("fetch", "Fetch a URL and return its main content as markdown.")
```

## Main

```lua
tools.add("search")
writer:infer("Search for: " .. args .. ". Return only the best URL, nothing else.")
var.url = reply

tools.add("fetch")
```

Fetch {{ var.url }} and write a detailed summary of its content.

```lua
store.write("summary.md", reply)
return reply
```
````

`model:infer(prompt)` calls the model from Lua. It:

- Snapshots the current tool bag (whatever `tools.add` has been called with so far)
- Runs the full tool loop
- Blocks until the model produces a final text response
- Sets the `reply` global
- Returns the response text

An optional second argument table is accepted today but ignored.

This enables **turn-gating** inside one open tool scope: add search, infer, add fetch, then fall through to final prose. `tools.add` remains legal until the first prose block closes tool scope. After the first prose, further `tools.add` calls fail.

`model:infer()` gives you explicit control over when inference happens, enabling turn-gated tool use before prose closes the scope.

## 13. Alternating Blocks

````markdown
---
name: alternating
description: Multi-turn within a single section.
promptforge: 1
---

# Alternating

```lua
writer = models.always("writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 32768 })
tools.need("search", "Search the web and return a list of results.")
tools.need("fetch", "Fetch a URL and return its main content as markdown.")
```

## Main

```lua
tools.add("search", "fetch")
```

Search for information about {{ args }}. Return the three most
relevant URLs, one per line, nothing else.

```lua
log("urls ready; continuing with fetch-capable tools already in scope")
```

Now fetch those URLs and write a comprehensive summary.
Cite each source.

```lua
store.write("report.md", reply)
return reply
```
````

A section can contain any number of alternating lua and prose blocks:

```
[lua] [prose] [lua] [prose] ... [lua]
```

The rules:

- **Non-final prose** - single-shot. One model round (may include tool calls in that turn), then control moves to the next lua block. The conversation accumulates.
- **Final prose** - full tool loop. The model keeps calling tools until it produces text. This text becomes `reply`.
- **Lua before the first prose** - may call `tools.add` / `models.use` / `model:infer`. Tool and model scope close on the first prose.
- **Lua after the first prose** - may inspect `reply`, write the store, `execute`, `jump`, `fanout`, and return. It may not call `tools.add` or `models.use`.

To change the tool bag between model turns, use `model:infer` in a single pre-prose Lua block (section 12), not `tools.add` between prose blocks.

Backward compatibility: the traditional prologue/prose/epilog pattern is exactly `[lua][prose][lua]`. It parses and runs identically.

Alternating blocks let you build multi-turn conversations within a single section, with Lua between turns under the scope rules above.

## 14. Composable Tool Sets

````markdown
---
name: composable_tools
description: Build tool sets conditionally.
promptforge: 1
---

# Composable Tools

```lua
models.always("writer",
    "A general-purpose model",
    { thinking = false, temperature = 0, context = 16384 })
search = tools.need("search",
    "Search the web and return a list of results.")
fetch = tools.need("fetch",
    "Fetch a URL and return its main content as markdown.")
```

```lua shared
research_tools = { "search", "fetch" }
fetch_only = { "fetch" }
```

## Main

```lua
if args:find("^http") then
    tools.add(fetch_only)
else
    tools.add(research_tools)
end
```

Analyze {{ args }}. If it is a URL, fetch and summarize it.
Otherwise, search for it, fetch the best result, and summarize.
````

Because Tool objects are first-class Lua values, you can:

- Store resolved aliases and serializable configuration in H1 `var`
- Pass arrays to `tools.add`
- Build conditional tool sets in the prologue based on `args` or other state
- Override `.description` on individual tools before adding them

Composable tool sets let you build, store, and conditionally select groups of tools as ordinary Lua values.

## 15. Sections as Subroutines: execute()

````markdown
---
name: pipeline
description: Execute sections as subroutines.
promptforge: 1
---

# Pipeline

```lua
models.always("writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 16384 })
tools.need("search", "Search the web and return a list of results.")
tools.need("fetch", "Fetch a URL and return its main content as markdown.")
```

## Research

```lua
tools.add("search", "fetch")
```

Research {{ args }}. Search for sources, fetch the best ones,
and write a detailed evidence summary. Cite URLs.

```lua
store.write("evidence.md", reply)
```

## Synthesize

```lua
var.evidence = store.inject("evidence.md")
```

Using only the evidence below, write a one-page briefing.

{{ var.evidence }}

## Main

```lua
local research = tasks["## Research"]
local evidence = execute(research)
local briefing = execute("## Synthesize")
return briefing
```
````

`execute(target, input?)` runs a named section as a subroutine. `target` is either a heading string with the `##` marker or a Section object from `tasks["## Name"]`. It:

- Creates a fresh VM (no Lua state from the caller)
- Creates a fresh conversation (no message history from the caller)
- Runs the full section lifecycle
- Returns the section's reply as a string

The called section shares the run's store, observer, execution id, gateway client, and tool registry. It gets a fresh copy of the captured H1 `var`, a fresh conversation, and a fresh VM. Optional `input` overrides `args` for the callee; omit it to inherit the caller's input.

`tasks["## Name"]` returns a Section object with `.name` and `.has_prose`. Missing headings are a hard error.

Recursion is capped at 8 levels. `jump` is not allowed inside `execute`.

`execute()` lets you call any section as a subroutine, with a fresh context, and get its reply back as a string.

## 16. Control Flow: jump()

````markdown
---
name: branching
description: Conditional section transfer.
promptforge: 1
---

# Branching

```lua
models.always("writer",
    "A general-purpose model",
    { thinking = false, temperature = 0, context = 8192 })
```

## Check

```lua
if args == "" then
    jump("## Help")
end
```

Analyze {{ args }} and determine if it is a valid topic for research.
Answer only "yes" or "no".

```lua
if reply:lower():find("no") then
    store.write("reason.md", reply)
    jump("## Reject")
end
jump("## Accept")
```

## Accept

The topic "{{ args }}" has been approved. Write a one-paragraph overview.

```lua
return reply
```

## Reject

```lua
var.reason = store.read("reason.md")
return "Rejected: " .. var.reason
```

## Help

```lua
return "Usage: provide a research topic as input."
```
````

`jump(target)` transfers control to a named section. `target` is a `##` heading string or a Section from `tasks`. It:

- Stops the current section immediately (later blocks in that section do not run)
- Clears the cross-section `reply` context
- Runs the named section next

Unlike `execute()`, `jump` does not return. Normal fall-through resumes from the jumped-to section.

A jump to a nonexistent section is a hard error.

`jump()` provides context-clearing transfer of control to another section, with no return to the caller.

## 17. Fanout (Parallel Execution)

````markdown
---
name: evidence_gatherer
description: Research multiple topics in parallel.
promptforge: 1
---

# Evidence Gatherer

```lua
models.always("writer",
    "A careful research model",
    { thinking = false, temperature = 0, context = 32768 })
tools.need("search", "Search the web and return a list of results.")
tools.need("fetch", "Fetch a URL and return its main content as markdown.")
```

## Main

```lua
local results = fanout("### Worker", "### Topics")
store.write("evidence.md", table.concat(results, "\n\n---\n\n"))
```

### Worker

```lua
tools.add("search", "fetch")
```

Research {{ item }} about {{ args }}.
Search for sources, fetch the best one, and write a summary
with the heading: ## {{ item }}

```lua
local s = tools.calls["search"] or 0
local f = tools.calls["fetch"] or 0
if s == 0 or f == 0 then
    return "## " .. tostring(item) .. "\n\nUNKNOWN"
end
```

### Topics

- Background and history
- Key people and leadership
- Recent news and developments
````

`fanout("### Worker", "### List")` runs the worker section once per item in the list, in parallel. Both heading arguments must include their `###` markers.

The **list section** is a list-only H3 with no Lua fences. Its prose contains only bullet items (unordered `- ` or `* `, or ordered `N. ` or `N) `). Markers are stripped. An empty list is a parse error.

The **worker section** is a normal template section. Each arm gets:

- A fresh VM
- `item` - the current list item text (also available as `{{ item }}` in prose)
- `sys.taskid` - the 1-based arm index ("1", "2", ...)
- Access to the shared store

Arms execute concurrently and share the run's store. The first arm error aborts all siblings.

`fanout` returns an ordered table of FanoutResult objects (list order, not finish order). Each result has:

| Field | Type | Description |
|---|---|---|
| `.text` | string | Arm reply text (or soft-degrade stub) |
| `.ok` | boolean | Whether the arm completed successfully |
| `.item` | string | The list item for this arm |
| `.exhausted` | boolean | True if the arm soft-degraded after tool-loop exhaustion |

`tostring(result)` returns `.text`. PromptForge wraps `table.concat` so concatenating a results table still works.

Children never execute by fall-through. Only an explicit `fanout()` call triggers child execution.

Fanout runs a worker section once per item in a list, in parallel, and returns ordered FanoutResult objects.

## 18. The sys Table

````markdown
---
name: sys_demo
description: Show sys table fields.
promptforge: 1
---

# Sys Demo

```lua
models.always("writer",
    "A general-purpose model",
    { thinking = false, temperature = 0, context = 8192 })
```

## Main

Write a one-sentence fact about {{ args }}.

```lua
store.write("report.md", reply
    .. "\n\n*Generated " .. sys.when
    .. " - " .. sys.model .. "*")
return store.read("report.md")
```
````

The `sys` table provides runtime metadata. It is sealed: reading an unknown key or writing any key raises an error.

| Field | Type | Available | Description |
|---|---|---|---|
| `sys.when` | string | Always (after host inject) | Run launch timestamp |
| `sys.now` | string | Always (after host inject) | Current section start time |
| `sys.id` | string | Always (after host inject) | 1-based section index |
| `sys.section_name` | string | Always (after host inject) | Current section name |
| `sys.execution` | string | Always (after host inject) | Run execution id |
| `sys.section_count` | string/number | Always (after host inject) | Top-level H2 count |
| `sys.model` | string | After scope close | Bound catalog model id |
| `sys.reply_finish_reason` | string | After a model turn | Last finish reason (for example `stop`) |
| `sys.taskid` | string | Fanout arms only | 1-based arm index |

`sys.model` becomes available after a model is selected for a prose turn. It is absent before that point in live H1 and section Lua.

`sys.when` is useful for report footers and provenance stamps. `sys.model` identifies which model produced the output.

The `sys` table provides sealed, read-only access to runtime metadata.

## 19. Error Handling and Validation

````markdown
---
name: validated
description: Validate tool usage and reply quality.
promptforge: 1
---

# Validated

```lua
models.always("writer",
    "A careful research model",
    { thinking = false, temperature = 0, context = 32768 })
tools.need("search", "Search the web and return a list of results.")
tools.need("fetch", "Fetch a URL and return its main content as markdown.")
```

## Main

```lua
tools.add("search", "fetch")
```

Research {{ args }}. Search for at least one source,
fetch it, and write a sourced summary.

```lua
local searches = tools.calls["search"] or 0
local fetches = tools.calls["fetch"] or 0

if searches == 0 or fetches == 0 then
    return "INCOMPLETE: search=" .. searches
        .. " fetch=" .. fetches
end

if not reply:find("http") then
    return "INCOMPLETE: no URLs cited in reply"
end

store.write("result.md", reply)
return reply
```
````

Post-prose Lua is your quality gate. Common validation patterns:

**Check that tools were called:**

```lua
assert(tools.calls["search"] > 0, "search was not called")
```

`tools.calls["alias"]` returns the count of model dispatches for that alias in this section. It counts intent - the tool is counted even if it errored. Indexing an alias not in scope is a hard error. Counts are installed after tool scope closes (first prose) and after `model:infer` installs them for that path.

**Check reply content:**

```lua
if reply:find("I don't know") or reply:find("I cannot") then
    return "INCOMPLETE: model declined"
end
```

**Return early on failure:**

A scalar return from any section Lua block ends the entire run. Use this to stop the pipeline when a section produces bad output rather than feeding garbage onward.

**Common error sources:**

- `{{ reply }}` in section 1 - hard error (nil)
- `{{ item }}` outside a fanout arm - hard error (nil)
- `tools.calls["unknown"]` - hard error naming the bad alias
- `tools.add` after the first prose - scope already closed
- Model called a tool not in scope - `Error::OutOfScopeToolCall`
- Non-empty prose without `models.use` or `models.always` - `Error::ModelRequired`
- Duplicate near-similar tools in effective scope - rejected before the model sees them

Post-prose Lua is your quality gate for validating tool usage, reply content, and pipeline integrity.

## 20. Capstone: A Complete Pipeline Prompt

````markdown
---
name: briefer
description: Generate a sourced briefing on any topic.
promptforge: 1
max_tool_iterations: 24
---

# Briefer

```lua
writer = models.always("writer",
    "A careful analysis model suited to structured reasoning",
    { thinking = false, temperature = 0, context = 32768 })
search = tools.need("search",
    "Search the web and return a list of results.")
fetch = tools.need("fetch",
    "Fetch a URL and return its main content as markdown.")
```

Generates a sourced briefing by gathering evidence in parallel,
then synthesizing a report.

## Main

```lua
local results = fanout("### Gather", "### Topics")
store.write("evidence.md", table.concat(results, "\n\n"))
local report = execute(tasks["## Report"])
return report
```

### Gather

```lua
tools.add(search, fetch)
```

Subject: {{ args }}
Section: {{ item }}

Search for information about the Subject relevant to this Section.
Fetch the best source. Write a summary under the heading ## {{ item }}.
Every claim needs a source URL.

```lua
local s = tools.calls["search"] or 0
local f = tools.calls["fetch"] or 0
if s == 0 or f == 0 then
    return "## " .. tostring(item) .. "\n\nUNKNOWN"
end
local text = reply:gsub("^%s*```.-\n", ""):gsub("\n```%s*$", "")
return text
```

### Topics

- Background and History
- Key People
- Recent Developments

## Report

```lua
var.evidence = store.inject("evidence.md")
```

Evidence packet:

{{ var.evidence }}

Write a structured briefing using ONLY the evidence above.
Do not invent facts. If something is missing, write UNKNOWN.

```lua
store.write("report.md", reply
    .. "\n\n*" .. sys.when .. " - " .. sys.model .. "*")
return reply
```
````

This prompt uses every major feature:

| Feature | Where |
|---|---|
| Live H1 | Resolves models and tools once |
| Tool objects | `search` and `fetch` globals passed to `tools.add` |
| Model object | `writer` returned by `models.always` |
| Fanout | `## Main` fans out `### Gather` over `### Topics` |
| FanoutResult | `table.concat` uses each result's text |
| `{{ item }}` | Each gather arm works on one topic |
| `tools.calls` | Gather epilog validates search and fetch |
| Store | Evidence written by fanout, read by Report |
| `store.inject` | Evidence injected with untrusted envelope |
| `execute()` / `tasks` | Main calls `## Report` as a subroutine |
| Template substitution | `{{ args }}`, `{{ item }}`, `{{ var.evidence }}` |
| `sys.when` / `sys.model` | Report footer with timestamp and model provenance |
| Scalar return | Report (and Main) return the final briefing |

The execution flow:

1. Live H1 runs once and captures model, tool, and `var` state
2. `## Main` Lua calls `fanout` - three gather arms run in parallel
3. Each arm searches, fetches, validates with `tools.calls`, returns evidence
4. `## Main` writes concatenated evidence to the store
5. `## Main` calls `execute(tasks["## Report"])`
6. `## Report` injects evidence, model writes briefing, epilog stamps and returns
7. `## Main` returns the report

A complete pipeline prompt combines live H1 resolution, tool objects, fanout, store, execute, substitution, validation, and sys metadata.

## 21. API Reference

### Globals

#### `args`

- **Type:** string
- **Available:** Live H1, section Lua, and prose substitution
- **Description:** The raw input string passed to `promptforge run <file> [input]`. Empty string if omitted.

```lua
var.subject = args
```

#### `reply`

- **Type:** string or nil
- **Available:** After a model turn in live H1 or the current section; prologue of sections 2+ (previous section's reply); prose substitution as `{{ reply }}`
- **Description:** The model's response text. Set after prose or `model:infer()`. Nil before the first model turn. Using `{{ reply }}` when nil is a hard error.

```lua
store.write("output.md", reply)
```

#### `item`

- **Type:** string or nil
- **Available:** Fanout worker sections only
- **Description:** The current fanout arm's item text. Using `{{ item }}` outside a fanout arm is a hard error.

```lua
log("Processing: " .. item)
```

#### `var`

- **Type:** table
- **Available:** Live H1, section Lua, and prose substitution
- **Description:** Serializable variable table. Live H1's final snapshot seeds each section; section and fanout mutations remain local. Scalars render as strings in substitution, tables render as JSON.

```lua
var.count = 5
var.tags = { "alpha", "beta" }
```

#### `sys`

- **Type:** sealed table
- **Available:** Live H1 and section Lua after host inject (read-only)
- **Description:** Runtime metadata. See section 18 for all fields.

```lua
log("Section " .. sys.id .. " at " .. sys.now)
```

#### `tasks`

- **Type:** table
- **Available:** Section Lua
- **Description:** Lookup of top-level H2 sections by heading string. `tasks["## Name"]` returns a Section object (`.name`, `.has_prose`) usable with `execute` and `jump`.

```lua
local step = tasks["## Research"]
local out = execute(step)
```

#### `log`

- **Type:** function
- **Signature:** `log(message: string)`
- **Returns:** nil
- **Available:** Live H1 and section Lua (a fresh callback is installed per phase)
- **Description:** Emits an observer checkpoint. The message must be a valid UTF-8 string, at most 256 characters, with no newlines or control characters. Use short static labels. Never log args, replies, tool data, credentials, paths, or store contents.

```lua
log("Research phase complete")
```

---

### Store Methods

All store methods are available in live H1 and section Lua after host injection.

#### `store.write`

- **Signature:** `store.write(path: string, contents: string)`
- **Returns:** nil
- **Description:** Create or overwrite a virtual file.

```lua
store.write("notes.md", "# Notes\n\n" .. reply)
```

#### `store.append`

- **Signature:** `store.append(path: string, contents: string)`
- **Returns:** nil
- **Description:** Append text to an existing file. Creates the file if it does not exist.

```lua
store.append("log.md", "\n" .. reply)
```

#### `store.read`

- **Signature:** `store.read(path: string) -> string`
- **Returns:** Verbatim file contents
- **Description:** Read a file's raw contents. Use for trusted Lua-side processing.

```lua
local text = store.read("notes.md")
```

#### `store.read_lines`

- **Signature:** `store.read_lines(path: string) -> string`
- **Returns:** Numbered lines in `N| content` format
- **Description:** Read a file with line numbers. Useful for editing and navigation with `str_replace`.

```lua
local numbered = store.read_lines("draft.md")
```

#### `store.inject`

- **Signature:** `store.inject(path: string) -> string`
- **Returns:** Contents wrapped in an untrusted nonce-framed envelope
- **Description:** Read a file for model-facing re-injection. The envelope marks the content as untrusted data.

```lua
var.evidence = store.inject("evidence.md")
```

#### `store.str_replace`

- **Signature:** `store.str_replace(path: string, old: string, new: string)`
- **Returns:** nil
- **Description:** Replace an exact text match in a file. The old string must appear exactly once.

```lua
store.str_replace("draft.md", "PLACEHOLDER", reply)
```

#### `store.delete`

- **Signature:** `store.delete(path: string)`
- **Returns:** nil
- **Description:** Remove a virtual file.

```lua
store.delete("temp.md")
```

#### `store.glob`

- **Signature:** `store.glob(pattern: string) -> table`
- **Returns:** Array of matching file paths
- **Description:** List store files matching a glob pattern.

```lua
local files = store.glob("arm-*.md")
```

#### `store.exists`

- **Signature:** `store.exists(path: string) -> boolean`
- **Returns:** true if the file exists
- **Description:** Check whether a virtual file exists.

```lua
if store.exists("cache.md") then
    var.cached = store.read("cache.md")
end
```

---

### Tool Functions

#### `tools.need`

- **Signature:** `tools.need(alias: string, description: string) -> Tool`
- **Returns:** Tool object
- **Available:** Live H1 only
- **Description:** Resolve a semantic tool capability immediately. The alias is case-sensitive (`[A-Za-z][A-Za-z0-9_-]{0,63}`). Returns a Tool object and records the frozen binding for sections.

```lua
search = tools.need("search",
    "Search the web and return a list of results.")
```

#### `tools.add`

- **Signature:** `tools.add(...)`
- **Returns:** nil
- **Available:** Section Lua before the first prose closes tool scope
- **Description:** Expose tools to the model. Accepts strings, Tool objects, and arrays of either. Only tools added here (plus `tools.always`) are visible to the model.

```lua
tools.add("search", "fetch")
tools.add(search)
tools.add({search, fetch})
```

#### `tools.always`

- **Signature:** `tools.always(alias: string)`
- **Returns:** nil
- **Available:** Live H1 only
- **Description:** Expose a declared tool in every model-facing section. The alias must have been declared with `tools.need` first.

```lua
tools.always("fetch")
```

#### `tools.calls`

- **Type:** table (read-only)
- **Available:** After tool counts are installed (`model:infer`, or after first prose closes scope)
- **Description:** Per-section count of model tool dispatches by alias. Indexing an out-of-scope alias is a hard error.

```lua
assert(tools.calls["search"] > 0, "search was never called")
```

---

### Model Functions

#### `models.need`

- **Signature:** `models.need(alias: string, description: string, opts?: table) -> Model`
- **Returns:** Model object
- **Available:** Live H1 only
- **Description:** Resolve a model capability immediately. Optional `opts`: `context` (minimum window), `thinking` (boolean), `temperature`, `max_tokens`.

```lua
writer = models.need("writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 32768 })
```

#### `models.always`

- **Signature:** `models.always(alias: string) -> Model` or `models.always(alias: string, description: string, opts?: table) -> Model`
- **Returns:** Model object (both forms)
- **Available:** Live H1 only
- **Description:** Set the prompt-wide default model. The single-argument form selects an already-declared alias. The three-argument form declares and selects. At most one `models.always` per prompt.

```lua
models.always("writer")

writer = models.always("writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 32768 })
```

#### `models.use`

- **Signature:** `models.use(alias: string)`
- **Returns:** nil
- **Available:** Section Lua before model scope closes (first prose)
- **Description:** Select a declared model for this section. Overrides `models.always`. At most one `models.use` per section.

```lua
models.use("fast")
```

#### `model:infer()`

- **Signature:** `model:infer(prompt: string, opts?: table) -> string`
- **Returns:** Model response text
- **Available:** Live H1 and section Lua (requires an execution infer hook)
- **Description:** Explicit model call from Lua. Snapshots the current tool bag, runs the full tool loop, sets `reply`, returns the text. An optional `opts` table is accepted but currently ignored.

```lua
tools.add("search")
writer:infer("Search for " .. args)
local search_results = reply

tools.add("fetch")
writer:infer("Fetch the best URL from: " .. search_results)
```

---

### Control Flow Functions

#### `execute()`

- **Signature:** `execute(target: string|Section, input?: string) -> string`
- **Returns:** The called section's reply
- **Available:** Section Lua
- **Description:** Run a named H2 section as a subroutine. Fresh VM and conversation. Shares store/observer/tools. Target is `## Name` or a Section from `tasks`. Recursion capped at 8. `jump` inside execute is rejected.

```lua
local analysis = execute("## Analyze")
local report = execute(tasks["## Report"])
```

#### `jump()`

- **Signature:** `jump(target: string|Section)`
- **Returns:** Does not return
- **Available:** Section Lua
- **Description:** Transfer control to a named section. Current section stops. Cross-section `reply` clears. Target is `## Name` or a Section from `tasks`.

```lua
if args == "" then
    jump("## Help")
end
```

#### `fanout()`

- **Signature:** `fanout(worker: string, list: string) -> table`
- **Returns:** Ordered table of FanoutResult objects
- **Available:** Section Lua of a parent section
- **Description:** Run the worker once per list item in parallel. Both arguments need `###` markers. List H3 must be list-only. Arms get `item` and `sys.taskid`. First arm error aborts siblings. `table.concat` coerces `.text`.

```lua
local results = fanout("### Worker", "### Topics")
store.write("all.md", table.concat(results, "\n\n"))
if not results[1].ok then
    log("arm soft-degraded")
end
```

---

### Objects

#### Tool

Returned by `tools.need()`.

| Property | Type | Mutable | Description |
|---|---|---|---|
| `.name` | string | No | Declared alias |
| `.description` | string | **Yes** | Model-facing description |
| `.parameters` | table | No | Parameter schema (empty object today) |
| `.wire_name` | string | No | Stable identity name |
| `.untrusted` | boolean | No | Untrusted flag (false on need handles today) |

```lua
fetch = tools.need("fetch", "Fetch a web page.")
fetch.description = "Fetch " .. args .. " and return markdown"
tools.add(fetch)
```

#### Model

Returned by `models.need()` or `models.always()`.

| Property | Type | Description |
|---|---|---|
| `.name` | string | Declared alias |
| `.model_id` | string | Resolved catalog model id |
| `.description` | string | Capability description |
| `.context` | number | Context window (tokens) |
| `.thinking` | boolean or nil | Frozen thinking preference |
| `.temperature` | number or nil | Frozen temperature |
| `.max_tokens` | number or nil | Frozen max tokens |
| `.dialect` | string | Tool dialect name |

All properties are read-only.

| Method | Signature | Description |
|---|---|---|
| `:infer()` | `model:infer(prompt, opts?) -> string` | Explicit inference from Lua (`opts` ignored today) |

```lua
writer = models.need("writer", "Analysis model",
    { temperature = 0, context = 32768 })
local result = writer:infer("Summarize: " .. args)
```

#### Section

Returned by `tasks["## Name"]`.

| Property | Type | Description |
|---|---|---|
| `.name` | string | Section heading name |
| `.has_prose` | boolean | Whether the section has model-facing prose |

Pass to `execute` or `jump` in place of a heading string.

#### FanoutResult

Returned as each element of a `fanout()` results table.

| Property | Type | Description |
|---|---|---|
| `.text` | string | Arm reply text |
| `.ok` | boolean | Success flag |
| `.item` | string | Source list item |
| `.exhausted` | boolean | Soft-degrade after tool-loop exhaustion |

`tostring(result)` equals `.text`.

---

### Frontmatter Fields

| Field | Required | Type | Description |
|---|---|---|---|
| `name` | Yes (parse) | string | Prompt identity |
| `description` | Yes (parse) | string | Human-readable description |
| `promptforge` | Yes (run) | integer | Engine major version (must be `1`) |
| `default_return` | No | string | Value returned when falling off the last section |
| `max_tool_iterations` | No | integer | Tool loop cap per section / infer (default 24) |

```yaml
---
name: my_prompt
description: Does something useful.
promptforge: 1
max_tool_iterations: 12
default_return: "No result produced."
---
```

---

### Prompt Structure Summary

````
---
frontmatter (YAML)
---

# Title (exactly one)

```lua
models.always("writer", "A capable model")
search = tools.need("search", "Search the web")
var.topic = args
```

H1 may continue with the exact alternating sequence:

```
[lua] [prose] [lua] [prose] ... [lua]
```

```lua shared
function normalize(text) return string.lower(text) end
```

H1 prose (executed in source order)

## Section (H2, runs top-to-bottom)

```lua
-- Lua before first prose: models.use, tools.add, infer, var
```

Prose with {{ substitution }} (sent to model; closes tool/model scope)

```lua
-- Lua after first prose: inspect reply, validate, store, return
```

## Another Section

-- Alternating blocks allowed:
-- [lua] [prose] [lua] [prose] ... [lua]
-- tools.add only before the first prose

### Child (H3, only for fanout)

```lua
-- Worker Lua
```

Worker prose with {{ item }}

### List (H3, list-only, for fanout)

- Item one
- Item two
- Item three
````

---

## 22. Quick Reference

> **Rules:**
>
> - Non-final prose blocks: single-shot. One model round (may include tool calls for that round). Control moves to the next lua block after the model responds. Conversation accumulates.
> - Final prose block: full tool loop. Model keeps calling tools until it produces text. That text becomes `reply`. Same as today.
> - Lua blocks: run sequentially. Can mutate tool scope (`tools.add`), write to store, inspect `reply`, call `execute()` or `jump()`, call `model:infer()` explicitly.
> - One conversation per section. Context grows across all blocks within the section. Cleared between sections.
> - Sections are subroutines. `execute("## Name", input?)` runs a section in a fresh VM, full tool loop, returns its reply. Like fanout but sequential and single.
> - `jump("## Name")` transfers control. Context clears. The current section stops. The named section runs next. No return to caller.

H1 and section block order is:

```
[lua] [prose] [lua] [prose] ... [lua]
```

Live H1 runs this sequence exactly once. The optional H1-only `lua shared` library is compiled as `Prompt.replay` and loaded into each fresh section VM before host injection. Host APIs and captured capability objects are unavailable while that library loads, so top-level host calls fail. Functions defined there can use host APIs later, when called from section code.

### Quick Reference Table

| Name | Kind | Available | Signature | Returns |
|---|---|---|---|---|
| `args` | global | Live H1, section Lua / prose | - | string |
| `reply` | global | After model turn; section 2+ prologue | - | string or nil |
| `item` | global | Fanout arms | - | string or nil |
| `var` | global | Live H1, section Lua / prose | - | table |
| `sys` | global | Live H1, section Lua (sealed) | - | table |
| `tasks` | global | Section Lua | `tasks["## Name"]` | Section |
| `log` | function | Live H1, section Lua | `log(msg)` | nil |
| `tools.need` | function | Live H1 | `tools.need(alias, desc)` | Tool |
| `tools.add` | function | Before first prose | `tools.add(...)` | nil |
| `tools.always` | function | Live H1 | `tools.always(alias)` | nil |
| `tools.calls` | table | After counts install | `tools.calls[alias]` | number |
| `models.need` | function | Live H1 | `models.need(alias, desc, opts?)` | Model |
| `models.always` | function | Live H1 | `models.always(...)` | Model |
| `models.use` | function | Before first prose | `models.use(alias)` | nil |
| `model:infer` | method | Live H1, section Lua | `model:infer(prompt, opts?)` | string |
| `execute` | function | Section Lua | `execute(target, input?)` | string |
| `jump` | function | Section Lua | `jump(target)` | never |
| `fanout` | function | Section Lua | `fanout(worker, list)` | FanoutResult[] |
| `store.*` | functions | Live H1, section Lua | see Store Methods | varies |

![Robot internals](images/banner-05.png)
