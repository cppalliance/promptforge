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

- **Frontmatter** - YAML between `---` fences. `name`, `description`, and `promptforge: 1` are required. The name must match `^[a-z][a-z0-9_]{0,47}$`.
- **H1 heading** - exactly one. This is the prompt's title.
- **H2 sections** - executable units. They run top to bottom.

Everything between the H1 and the first H2 is a human-readable description. It is not sent to the model.

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

`models.use("writer")` selects a model binding declared in the preamble (covered next). Without a preamble declaring models, this would fail. For now, note the pattern: the prologue sets up what the model turn needs.

The prologue runs Lua before the model sees the prose.

## 4. The Preamble (H1 Shared Lua)

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

## Main

```lua
tools.add("fetch")
```

Fetch {{ args }} and summarize its content in three bullet points.
````

The fenced `lua` block directly under the H1 is the **preamble**. It runs once at startup. Declarations made here are available to every section.

`models.always` does three things in one call: declares a model need, resolves it against the gateway catalog, and sets it as the default for all sections. Sections that call `models.use` override this default. The combined form takes an alias, a capability description, and an optional table of constraints.

`tools.need` declares a semantic capability need. The alias `"fetch"` is your local name. The description tells the tool picker what you need - it resolves this against the live tool registry. Declaring a need does not expose the tool to the model. That requires `tools.add` in a section prologue.

The preamble also accepts `models.need` (declare without selecting as default) and `tools.always` (expose a tool in every section). Shared functions defined in the preamble are available in every section's prologue and epilog.

The preamble declares tools and models once, making them available to every section.

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

The epilog shares the same VM as the prologue - variables set in the prologue are still accessible. You can:

- Inspect and transform `reply`
- Write to the store
- Check `tools.calls` counts
- Return a value to end the run

A `return` from the epilog ends the entire run with that value. If you do not return, execution falls through to the next section.

A section's three phases are: prologue (setup), prose (model turn), epilog (post-processing). All three are optional.

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

- A fresh Lua VM (no Lua state carries over)
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

The store is a run-scoped virtual filesystem. Files exist only for the duration of the run and are shared across all sections.

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

Five namespaces are available in prose substitution:

| Placeholder | Source |
|---|---|
| `{{ args }}` | The raw input string |
| `{{ reply }}` | Previous section's model reply (nil in section 1) |
| `{{ var.x }}` | Values set in the prologue's `var` table |
| `{{ sys.when }}` | Run launch timestamp |
| `{{ sys.now }}` | Current section start time |
| `{{ sys.id }}` | 1-based section index |
| `{{ sys.model }}` | Bound model catalog id (after scope close) |
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

The loop is capped at `max_tool_iterations` (default 24) per section to prevent runaway loops. Set it in frontmatter to change the cap.

`tools.add` in the prologue takes one or more alias strings. Only tools added in the current section's prologue (plus any `tools.always` from the preamble) are visible to the model.

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
local search = tools.need("search",
    "Search the web and return a list of results.")
local fetch = tools.need("fetch",
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

`tools.need` returns a Tool object. You can inspect its properties:

| Property | Type | Description |
|---|---|---|
| `.name` | string | The resolved tool name |
| `.description` | string | Model-facing description (mutable) |
| `.parameters` | table | JSON schema of accepted parameters |
| `.wire_name` | string | Transport-level name |
| `.untrusted` | boolean | Whether results are marked untrusted |

The `.description` property is mutable. Changing it before `tools.add` controls what the model sees. This lets you customize the tool's description per section or per input.

`tools.add` accepts Tool objects, strings, and arrays of either:

```lua
tools.add(search)                    -- single object
tools.add("search", "fetch")        -- strings (as before)
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
local writer = models.need("writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 32768 })
local fast = models.need("fast",
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

`models.need` and `models.always` return a Model object. Properties:

| Property | Type | Description |
|---|---|---|
| `.name` | string | The alias you declared |
| `.model_id` | string | Resolved catalog model id |
| `.description` | string | Capability description |
| `.context` | number | Context window size in tokens |
| `.thinking` | string | `"never"`, `"always"`, or `"switchable"` |
| `.temperature` | number | Frozen temperature value |
| `.max_tokens` | number | Frozen max tokens value |
| `.dialect` | string | Tool dialect name |

All properties are read-only. The Model object represents a frozen binding - its parameters were locked at bind time and cannot change during execution.

You can declare multiple models with `models.need` and select between them per section with `models.use`. `models.always` sets the prompt-wide default.

Model objects let you inspect bound model properties and select between multiple declared models.

## 12. Explicit Inference with model:infer()

````markdown
---
name: gated_research
description: Search first, then fetch, then summarize.
promptforge: 1
---

# Gated Research

```lua
local writer = models.always("writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 32768 })
tools.need("search", "Search the web and return a list of results.")
tools.need("fetch", "Fetch a URL and return its main content as markdown.")
```

## Main

```lua
tools.add("search")
writer:infer("Search for: " .. args .. ". Return the best URL.")
local url = reply

tools.add("fetch")
```

Fetch {{ var.url }} and write a detailed summary of its content.

```lua
store.write("summary.md", reply)
return reply
```
````

Wait - there is an error in that example. Let me fix the pattern. The prologue calls `writer:infer()`, which runs a full tool loop, sets `reply`, and returns the text. Then it scopes fetch and falls through to prose.

Corrected prologue:

```lua
tools.add("search")
writer:infer("Search for: " .. args .. ". Return only the best URL, nothing else.")
var.url = reply

tools.add("fetch")
```

`model:infer(prompt)` calls the model from Lua. It:

- Takes the current tool scope (whatever `tools.add` has been called with)
- Runs the full tool loop (model calls tools, gets results, continues)
- Blocks until the model produces a final text response
- Sets the `reply` global
- Returns the response text

This enables **turn-gating**: scope search, infer to get URLs, then scope fetch, and let the final prose do the detailed work. Without `infer`, the model would see both tools at once and might interleave them unpredictably.

`model:infer()` snapshots the current tool bag. Calls to `tools.add` after an infer affect the next infer or the final prose, not the one in progress.

`model:infer()` gives you explicit control over when inference happens, enabling turn-gated tool use.

## 13. Alternating Blocks

````markdown
---
name: alternating
description: Multi-turn within a single section.
promptforge: 1
---

# Alternating

```lua
local writer = models.always("writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 32768 })
tools.need("search", "Search the web and return a list of results.")
tools.need("fetch", "Fetch a URL and return its main content as markdown.")
```

## Main

```lua
tools.add("search")
```

Search for information about {{ args }}. Return the three most
relevant URLs, one per line, nothing else.

```lua
tools.add("fetch")
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

- **Non-final prose** - single-shot. One model round (may include tool calls for that round), then control moves to the next lua block. The conversation accumulates.
- **Final prose** - full tool loop. The model keeps calling tools until it produces text. This text becomes `reply`. Same behavior as sections with a single prose block.
- **Lua blocks** - run sequentially. Can mutate tool scope, write to store, inspect `reply` from a previous prose block or `infer()`, call `execute()` or `goto()`.

The conversation accumulates within the section. Each prose block's response is added to the history, so the model in later blocks sees everything that came before.

Backward compatibility: the traditional prologue/prose/epilog pattern is exactly `[lua][prose][lua]` - one lua, one final prose, one lua. It parses and runs identically.

Alternating blocks let you build multi-turn conversations within a single section, with Lua control between each turn.

## 14. Composable Tool Sets

````markdown
---
name: composable_tools
description: Build tool sets conditionally.
promptforge: 1
---

# Composable Tools

```lua
local writer = models.always("writer",
    "A general-purpose model",
    { thinking = false, temperature = 0, context = 16384 })
local search = tools.need("search",
    "Search the web and return a list of results.")
local fetch = tools.need("fetch",
    "Fetch a URL and return its main content as markdown.")

var.research_tools = { search, fetch }
var.fetch_only = { fetch }
```

## Main

```lua
if args:find("^http") then
    tools.add(var.fetch_only)
else
    tools.add(var.research_tools)
end
```

Analyze {{ args }}. If it is a URL, fetch and summarize it.
Otherwise, search for it, fetch the best result, and summarize.
````

Because Tool objects are first-class Lua values, you can:

- Store them in arrays
- Store them in the `var` table
- Pass arrays to `tools.add`
- Build conditional tool sets based on `args` or other state
- Override `.description` on individual tools before adding them

Tool objects compose with standard Lua data structures. Build tool sets in the preamble and select them conditionally in the prologue.

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
local writer = models.always("writer",
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
local evidence = execute("## Research")
local briefing = execute("## Synthesize")
return briefing
```
````

`execute("## Section Name")` runs a named section as a subroutine. It:

- Creates a fresh VM (no Lua state from the caller)
- Creates a fresh conversation (no message history from the caller)
- Runs the full section lifecycle (prologue, prose with tool loop, epilog)
- Returns the section's reply as a string

The called section shares the run's store, observer, execution id, gateway client, and tool registry. It gets fresh `var`, a fresh conversation, and a fresh VM.

The heading argument must include the `##` marker: `execute("## Research")`, not `execute("Research")`.

Recursion is capped at 8 levels to prevent infinite loops.

`execute()` lets you call any section as a subroutine, with a fresh context, and get its reply back as a string.

## 16. Control Flow: goto()

````markdown
---
name: branching
description: Conditional section transfer.
promptforge: 1
---

# Branching

```lua
local writer = models.always("writer",
    "A general-purpose model",
    { thinking = false, temperature = 0, context = 8192 })
```

## Check

```lua
if args == "" then
    goto("## Help")
end
```

Analyze {{ args }} and determine if it is a valid topic for research.
Answer only "yes" or "no".

```lua
if reply:lower():find("no") then
    store.write("reason.md", reply)
    goto("## Reject")
end
goto("## Accept")
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

`goto("## Section Name")` transfers control to a named section. It:

- Stops the current section immediately (no epilog runs after goto)
- Clears the conversation context
- Runs the named section next

Unlike `execute()`, goto does not return. The current section ends. The named section becomes the next section in the execution sequence. Normal fall-through resumes from that point.

The heading argument must include the `##` marker. A goto to a nonexistent section is a hard error.

`goto()` provides context-clearing transfer of control to another section, with no return to the caller.

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

The **list section** is a list-only H3 with no prologue or epilog. Its prose contains only bullet items (unordered `- ` or `* `, or ordered `N. ` or `N) `). Markers are stripped. An empty list is a parse error.

The **worker section** is a normal template section. Each arm gets:

- A fresh VM
- `item` - the current list item text (also available as `{{ item }}` in prose)
- `sys.taskid` - the 1-based arm index ("1", "2", ...)
- Access to the shared store

Arms execute concurrently and share the run's store. The first arm error aborts all siblings. `fanout` returns an ordered Lua table of arm replies (list order, not finish order).

Children never execute by fall-through. Only an explicit `fanout()` call triggers child execution.

Fanout runs a worker section once per item in a list, in parallel, and returns an ordered table of replies.

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
| `sys.when` | string | Always | Run launch timestamp |
| `sys.now` | string | Always | Current section start time |
| `sys.id` | string | Always | 1-based section index |
| `sys.model` | string | After scope close | Bound catalog model id |
| `sys.taskid` | string | Fanout arms only | 1-based arm index |

`sys.model` is not available in the preamble or in H1 shared Lua because the model binding has not been selected yet. It becomes available after the section's scope closes (in prose substitution and the epilog).

`sys.when` is useful for report footers and provenance stamps. `sys.model` identifies which model produced the output.

The `sys` table provides sealed, read-only access to runtime metadata like timestamps, section index, and model identity.

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

The epilog is your quality gate. Common validation patterns:

**Check that tools were called:**

```lua
assert(tools.calls["search"] > 0, "search was not called")
```

`tools.calls["alias"]` returns the count of model dispatches for that alias in this section. It counts intent - the tool is counted even if it errored. Indexing an alias not in scope is a hard error.

**Check reply content:**

```lua
if reply:find("I don't know") or reply:find("I cannot") then
    return "INCOMPLETE: model declined"
end
```

**Return early on failure:**

A scalar return from the epilog ends the entire run. Use this to stop the pipeline when a section produces bad output rather than feeding garbage to the next section.

**Common error sources:**

- `{{ reply }}` in section 1 - hard error (nil)
- `{{ item }}` outside a fanout arm - hard error (nil)
- `tools.calls["unknown"]` - hard error naming the bad alias
- Model called a tool not in scope - `Error::OutOfScopeToolCall`
- Non-empty prose without `models.use` or `models.always` - `Error::ModelRequired`
- Duplicate near-similar tools in effective scope - rejected before the model sees them

The epilog is your quality gate for validating tool usage, reply content, and pipeline integrity.

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
local writer = models.always("writer",
    "A careful analysis model suited to structured reasoning",
    { thinking = false, temperature = 0, context = 32768 })
local search = tools.need("search",
    "Search the web and return a list of results.")
local fetch = tools.need("fetch",
    "Fetch a URL and return its main content as markdown.")
```

Generates a sourced briefing by gathering evidence in parallel,
then synthesizing a report.

## Main

```lua
local results = fanout("### Gather", "### Topics")
store.write("evidence.md", table.concat(results, "\n\n"))
local report = execute("## Report")
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
| Preamble | H1 shared Lua declares model and tools |
| Tool objects | `search` and `fetch` stored as variables, passed to `tools.add` |
| Model object | `writer` returned by `models.always` |
| Fanout | `## Main` fans out `### Gather` over `### Topics` |
| `{{ item }}` | Each gather arm works on one topic |
| `tools.calls` | Epilog validates that search and fetch were called |
| Store | Evidence written by fanout, read by Report |
| `store.inject` | Evidence injected with untrusted envelope |
| `execute()` | Main calls `## Report` as a subroutine |
| Template substitution | `{{ args }}`, `{{ item }}`, `{{ var.evidence }}` |
| `sys.when` / `sys.model` | Report footer with timestamp and model provenance |
| Epilog return | Report returns the final briefing |

The execution flow:

1. Preamble runs once, declares writer/search/fetch
2. `## Main` prologue calls `fanout` - three gather arms run in parallel
3. Each arm searches, fetches, validates with `tools.calls`, returns evidence
4. `## Main` prologue writes concatenated evidence to store
5. `## Main` prologue calls `execute("## Report")`
6. `## Report` injects evidence, model writes briefing, epilog stamps and returns
7. `## Main` prologue returns the report

A complete pipeline prompt combines preamble, tool objects, model objects, fanout, store, execute, substitution, epilog validation, and sys metadata.

## 21. API Reference

### Globals

#### `args`

- **Type:** string
- **Available:** Always (prologue, epilog, prose substitution)
- **Description:** The raw input string passed to `promptforge run <file> [input]`. Empty string if omitted.

```lua
var.subject = args
```

#### `reply`

- **Type:** string or nil
- **Available:** Epilog (current section's model reply); prologue of sections 2+ (previous section's reply); prose substitution as `{{ reply }}`
- **Description:** The model's response text. Set after a model turn completes (from prose or `model:infer()`). Nil in section 1's prologue. Using `{{ reply }}` when nil is a hard error.

```lua
store.write("output.md", reply)
```

#### `item`

- **Type:** string or nil
- **Available:** Fanout worker sections only (prologue, epilog, prose substitution as `{{ item }}`)
- **Description:** The current fanout arm's item text. Nil outside fanout arms. Using `{{ item }}` outside a fanout arm is a hard error.

```lua
log("Processing: " .. item)
```

#### `var`

- **Type:** table
- **Available:** Always (prologue, epilog, prose substitution as `{{ var.x }}`)
- **Description:** Section-local variable table. Values set here are accessible in prose substitution and the epilog. Fresh per section and per fanout arm. Scalars render as strings in substitution, tables render as JSON.

```lua
var.count = 5
var.tags = { "alpha", "beta" }
```

#### `sys`

- **Type:** sealed table
- **Available:** Always (read-only; unknown reads and any writes raise)
- **Description:** Runtime metadata. See section 18 for all fields.

| Field | Type | Available | Value |
|---|---|---|---|
| `sys.when` | string | Always | Run launch timestamp |
| `sys.now` | string | Always | Current section start time |
| `sys.id` | string | Always | 1-based section index |
| `sys.model` | string | After scope close | Bound catalog model id |
| `sys.taskid` | string | Fanout arms | 1-based arm index |

```lua
log("Section " .. sys.id .. " at " .. sys.now)
```

#### `log`

- **Type:** function
- **Signature:** `log(message: string)`
- **Returns:** nil
- **Available:** Preamble, prologue, epilog (a fresh callback is installed per phase)
- **Description:** Emits an observer checkpoint. The message must be a valid UTF-8 string, at most 256 characters, with no newlines or control characters. Use short static labels. Never log args, replies, tool data, credentials, paths, or store contents.

```lua
log("Research phase complete")
```

---

### Store Methods

All store methods are available in prologue, epilog, and preamble.

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
- **Description:** Read a file for model-facing re-injection. The envelope marks the content as untrusted data, preventing prompt injection from stored content.

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
- **Available:** Preamble only
- **Description:** Declare a semantic tool capability need. The alias is your local name (case-sensitive, `[A-Za-z][A-Za-z0-9_-]{0,63}`). The description tells the picker what you need. The picker resolves this against the live tool registry at bind time. Returns a Tool object you can inspect and pass to `tools.add`.

```lua
local search = tools.need("search",
    "Search the web and return a list of results.")
```

#### `tools.add`

- **Signature:** `tools.add(aliases_or_objects: ...)`
- **Returns:** nil
- **Available:** Prologue only
- **Description:** Expose tools to the model for this section. Accepts strings, Tool objects, and arrays of either. Only tools added here (plus `tools.always` from the preamble) are visible to the model.

```lua
tools.add("search", "fetch")
tools.add(search_tool)
tools.add({search_tool, fetch_tool})
```

#### `tools.always`

- **Signature:** `tools.always(alias: string)`
- **Returns:** nil
- **Available:** Preamble only
- **Description:** Expose a declared tool in every model-facing section. Use only when every section genuinely needs this tool. The alias must have been declared with `tools.need` first.

```lua
tools.always("fetch")
```

#### `tools.calls`

- **Type:** table (read-only)
- **Available:** Prologue and epilog (after model turns)
- **Description:** Per-section count of model tool dispatches by alias. Pre-seeded at 0 for every in-scope alias. Indexing an out-of-scope alias is a hard error that names the bad alias and lists valid ones.

```lua
assert(tools.calls["search"] > 0, "search was never called")
```

---

### Model Functions

#### `models.need`

- **Signature:** `models.need(alias: string, description: string, opts?: table) -> Model`
- **Returns:** Model object
- **Available:** Preamble only
- **Description:** Declare a model capability need. The description is matched against the gateway catalog. Optional `opts` table: `context` (minimum window, filters catalog), `thinking` (boolean, filters and freezes), `temperature` (frozen onto binding), `max_tokens` (frozen onto binding). Returns a Model object.

```lua
local writer = models.need("writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 32768 })
```

#### `models.always`

- **Signature:** `models.always(alias: string)` or `models.always(alias: string, description: string, opts?: table) -> Model`
- **Returns:** Model object (combined form) or nil (selection-only form)
- **Available:** Preamble only
- **Description:** Set the prompt-wide default model. The single-argument form selects an already-declared alias. The three-argument combined form declares and selects in one call (sugar for `models.need` + `models.always`). At most one `models.always` per prompt.

```lua
models.always("writer")

-- or combined form:
local writer = models.always("writer",
    "A careful analysis model",
    { thinking = false, temperature = 0, context = 32768 })
```

#### `models.use`

- **Signature:** `models.use(alias: string)`
- **Returns:** nil
- **Available:** Prologue only
- **Description:** Select a declared model for this section. Overrides the `models.always` default. At most one `models.use` per section.

```lua
models.use("fast")
```

#### `model:infer()`

- **Signature:** `model:infer(prompt: string, opts?: table) -> string`
- **Returns:** Model response text
- **Available:** Prologue (any lua block in a section)
- **Description:** Explicitly call the model from Lua. Snapshots the current tool bag, runs the full tool loop, blocks until the model produces text, sets `reply`, and returns the response. Use for turn-gating (scope tools, infer, scope different tools, infer again or fall through to prose).

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

- **Signature:** `execute(section_name: string, input?: string) -> string`
- **Returns:** The called section's reply
- **Available:** Prologue, epilog (any lua block)
- **Description:** Run a named H2 section as a subroutine. Fresh VM, fresh conversation, full tool loop. Shares the run's store, observer, and tool registry. The heading argument must include the `##` marker. Recursion capped at 8 levels.

```lua
local analysis = execute("## Analyze")
store.write("analysis.md", analysis)
```

#### `goto()`

- **Signature:** `goto(section_name: string)`
- **Returns:** Does not return
- **Available:** Prologue, epilog (any lua block)
- **Description:** Transfer control to a named section. The current section stops immediately (no epilog runs after goto). Context clears, conversation resets. The heading argument must include the `##` marker. Goto to a nonexistent section is a hard error.

```lua
if args == "" then
    goto("## Help")
end
```

#### `fanout()`

- **Signature:** `fanout(worker: string, list: string) -> table`
- **Returns:** Ordered Lua table of arm reply strings (list order)
- **Available:** Prologue, epilog of a parent section
- **Description:** Run the worker section once per item in the list section, in parallel. Both arguments must include their heading markers (`###`). The list section must be a list-only H3 sibling (no prologue, no epilog). Arms get `item` and `sys.taskid`. First arm error aborts siblings.

```lua
local results = fanout("### Worker", "### Topics")
store.write("all.md", table.concat(results, "\n\n"))
```

---

### Objects

#### Tool

Returned by `tools.need()`.

| Property | Type | Mutable | Description |
|---|---|---|---|
| `.name` | string | No | Resolved tool name |
| `.description` | string | **Yes** | Model-facing description |
| `.parameters` | table | No | JSON schema of parameters |
| `.wire_name` | string | No | Transport-level name |
| `.untrusted` | boolean | No | Whether results are untrusted |

```lua
local fetch = tools.need("fetch", "Fetch a web page.")
fetch.description = "Fetch " .. args .. " and return markdown"
tools.add(fetch)
```

#### Model

Returned by `models.need()` or `models.always()` (combined form).

| Property | Type | Description |
|---|---|---|
| `.name` | string | The alias declared |
| `.model_id` | string | Resolved catalog model id |
| `.description` | string | Capability description |
| `.context` | number | Context window (tokens) |
| `.thinking` | string | `"never"`, `"always"`, or `"switchable"` |
| `.temperature` | number | Frozen temperature |
| `.max_tokens` | number | Frozen max tokens |
| `.dialect` | string | Tool dialect name |

All properties are read-only.

| Method | Signature | Description |
|---|---|---|
| `:infer()` | `model:infer(prompt, opts?) -> string` | Explicit inference from Lua |

```lua
local writer = models.need("writer", "Analysis model",
    { temperature = 0, context = 32768 })
local result = writer:infer("Summarize: " .. args)
```

---

### Frontmatter Fields

| Field | Required | Type | Description |
|---|---|---|---|
| `name` | Yes | string | Prompt identity (`^[a-z][a-z0-9_]{0,47}$`) |
| `description` | Yes | string | Human-readable description |
| `promptforge` | Yes | integer | Engine major version (must be `1`) |
| `default_return` | No | string | Value returned when falling off the last section |
| `max_tool_iterations` | No | integer | Tool loop cap per section (default 24) |

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
-- Preamble: shared Lua (runs once)
-- tools.need, tools.always, models.need, models.always
-- Shared functions available to all sections
```

Description text (not executed)

## Section (H2, runs top-to-bottom)

```lua
-- Prologue: section setup
-- models.use, tools.add, var assignments
```

Prose with {{ substitution }} (sent to model)

```lua
-- Epilog: post-processing
-- inspect reply, validate, store.write, return
```

## Another Section

-- Alternating blocks allowed:
-- [lua] [prose] [lua] [prose] ... [lua]

### Child (H3, only for fanout)

```lua
-- Worker prologue
```

Worker prose with {{ item }}

```lua
-- Worker epilog
```

### List (H3, list-only, for fanout)

- Item one
- Item two
- Item three
````

---

### Quick Reference Table

| Name | Kind | Available | Signature | Returns |
|---|---|---|---|---|
| `args` | global | Always | - | string |
| `reply` | global | Epilog, section 2+ prologue | - | string or nil |
| `item` | global | Fanout arms | - | string or nil |
| `var` | global | Always | - | table |
| `sys` | global | Always (sealed) | - | table |
| `log` | function | Preamble, prologue, epilog | `log(msg)` | nil |
| `tools.need` | function | Preamble | `tools.need(alias, desc)` | Tool |
| `tools.add` | function | Prologue | `tools.add(...)` | nil |
| `tools.always` | function | Preamble | `tools.always(alias)` | nil |
| `tools.calls` | table | Prologue, epilog | `tools.calls[alias]` | number |
| `models.need` | function | Preamble | `models.need(alias, desc, opts?)` | Model |
| `models.always` | function | Preamble | `models.always(alias, ...)` | Model or nil |
| `models.use` | function | Prologue | `models.use(alias)` | nil |
| `model:infer` | method | Lua blocks | `model:infer(prompt, opts?)` | string |
| `execute` | function | Lua blocks | `execute(name, input?)` | string |
| `goto` | function | Lua blocks | `goto(name)` | never |
| `fanout` | function | Prologue, epilog | `fanout(worker, list)` | table |
| `store.write` | function | Always | `store.write(path, text)` | nil |
| `store.append` | function | Always | `store.append(path, text)` | nil |
| `store.read` | function | Always | `store.read(path)` | string |
| `store.read_lines` | function | Always | `store.read_lines(path)` | string |
| `store.inject` | function | Always | `store.inject(path)` | string |
| `store.str_replace` | function | Always | `store.str_replace(path, old, new)` | nil |
| `store.delete` | function | Always | `store.delete(path)` | nil |
| `store.glob` | function | Always | `store.glob(pattern)` | table |
| `store.exists` | function | Always | `store.exists(path)` | boolean |
