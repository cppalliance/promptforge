# What a Prompt Is

A PromptForge prompt is a single Markdown file that is also a program: the prose you want a model to see sits right next to the Lua that decides what happens to it, and a host runs the whole file for you. This chapter shows you the shape of that file, the smallest prompt that runs, how prose and Lua meet, how sections work together, and what the host does before and during a run, so you can write and run a first prompt today and know which chapter to open next for each part.

## What a prompt file is

A prompt file is one Markdown file (`.md`) that a host parses and runs. The host is the program that runs your prompt and does its outside work for it. You never call the host directly: you write the file, and the host runs it. The prompt is the program, with Lua holding the logic and prose holding text that Lua can read and send to a model.

A prompt file has three parts, in this order:

- Frontmatter: a YAML block between two `---` lines at the top of the file, holding at least `name`, `description`, and `promptforge: 0`.
- One H1 title: a `# ` line with non-empty text.
- Sections: the headings after the H1 at level two or deeper, starting with `##`. Each section holds Markdown prose, `lua` fences with the prompt's logic, or both.

Here is a complete prompt file:

````markdown
---
name: greeter
description: says hi
promptforge: 0
---

# Greeter

## Say hi

Say hello.
````

Its frontmatter is the three lines between the `---` delimiters, its H1 title is `Greeter`, and it has one section, `## Say hi`, holding one line of prose. The delimiters and keys are covered in [the frontmatter header](02-file-structure.md#the-frontmatter-header), and the two required strings in [name and description](02-file-structure.md#name-and-description).

A file with no `---` delimiters at the top, or with YAML between them that does not parse, fails with the parse error kind `Frontmatter` ([parse error kinds](17-limits-and-errors.md#parse-error-kinds)).

The title is exactly one H1 with non-empty text: a file with no H1 fails with the parse error kind `Structure` and `prompt requires an H1 title`, a file with several gives `prompt must contain exactly one H1 title`, and a blank title gives `prompt H1 title must not be empty` ([the one-H1 rule](02-file-structure.md#what-a-prompt-file-looks-like)).

Sections nest by heading level, so a `###` heading under a `##` section starts a child section, as [sections and nesting](02-file-structure.md#sections-and-nesting) describes. Inside a section, prose and `lua` fences alternate into prose blocks and Lua blocks, which [Lua blocks and prose blocks](03-blocks-and-prose.md#lua-blocks-and-prose-blocks) teaches.

The `promptforge: 0` line is what marks a Markdown file as a PromptForge prompt. `0` is the version this build runs, and the run checks it before any section runs. Two cases stop a run at that check:

- Any other version number fails the run with the run error kind `Version` and `unsupported promptforge version: {n} (this build supports major 0)`, where `{n}` is the declared number.
- A file with no `promptforge:` key still parses, but the run declines it with the parse error kind `Structure` and `not a promptforge prompt: no promptforge version`, an error that names the prompt's `name`.

So a file with the right layout but no `promptforge: 0` parses and never runs, and every prompt that runs declares `promptforge: 0`. [The promptforge version](02-file-structure.md#the-promptforge-version) covers the key in full, [parse error kinds](17-limits-and-errors.md#parse-error-kinds) lists `Structure`, and [how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified) lists `Version` with the other run error kinds.

The part between the H1 title and the first section is the H1 body, and it can hold prose and `lua` fences too ([the H1 title and its content](02-file-structure.md#the-h1-title-and-its-content)). Because the H1 body can do all the work, a prompt needs no minimum number of sections. This prompt has none, and its run ends with the result `hello`:

````markdown
---
name: h1-only
description: Does all its work in the H1 body
promptforge: 0
---

# H1 Only

```lua
return "hello"
```
````

````text
hello
````

With `local x = 1` in place of the `return`, the same prompt ends with `done`. Blocks in the H1 body run in [the H1 pass](04-how-a-prompt-runs.md#the-h1-pass), before any section.

## The smallest complete prompt

The greeter above is already a complete prompt. It has one section, no Lua anywhere, and nothing in its H1 body, and it declares nothing the host has to provide. Run it, and it ends like this:

````text
done
````

No model is called. No Lua reads the section's prose, so the prose is discarded unread ([pending prose](03-blocks-and-prose.md#the-pending-prose-buffer)). The section ends without a result, and because no section returns anything, [the run](04-how-a-prompt-runs.md#what-a-run-does) ends with the fallback result `done` ([block and section returns](04-how-a-prompt-runs.md#block-and-section-returns)).

To compute a result instead, add one `lua` fence to the section:

````markdown
---
name: greeter
description: Returns a greeting
promptforge: 0
---

# Greeter

## Say hi

```lua
return 'hello'
```
````

````text
hello
````

The fence's `return` value becomes the section's result, and because this section's return ends the run, it is also the run's result text. The prompt declares no model, no tools, and no capabilities, and needs none: a Lua block that returns a string does all the work itself. A literal `return` asks the host for no work at all, so the run never waits on anything outside the prompt. Tools that Lua never calls change nothing either: with [tool slots](#the-prompt-and-its-host) filled but never called, a block of `return 'plain'` still ends the run with `plain`.

A prompt can also return the argument string the run received, which Lua reads as `args` ([input basics](06-arguments.md#input-basics)). This prompt ends with whatever text it was given:

````markdown
---
name: echo
description: Return the input argument unchanged
promptforge: 0
---

# Echo

## Main

```lua
return args
```
````

## Prose and Lua

Prose becomes useful when a Lua block reads it. Write a line of prose, then a `lua` fence below it in the same section:

````markdown
---
name: read-prose
description: Returns the prose written above its block
promptforge: 0
---

# Read Prose

## Main

Tell the reader what this prompt does.

```lua
return prose
```
````

````text
Tell the reader what this prompt does.
````

The prose written since the heading or the previous fence is pending prose, and it builds up for the next Lua block ([pending prose](03-blocks-and-prose.md#the-pending-prose-buffer)). That block reads it as the read-only `prose` global ([the prose global](03-blocks-and-prose.md#the-prose-global)), so `return prose` makes the prose the section's result.

A `{{ var.word }}` placeholder fills in a value the first time a block reads the prose, not before ([what substitution does](07-substitution.md#what-substitution-does)). `var` is a table that keeps values from block to block ([keeping values in var](05-lua-environment.md#keeping-values-in-var)), so a block that sets `var.word` first and then reads `prose` sees its own value:

````markdown
---
name: late-render
description: Renders its prose at the first read
promptforge: 0
---

# Late Render

## Only

The word is {{ var.word }}.

```lua
var.word = 'mutated'
return prose
```
````

````text
The word is mutated.
````

Prose alone never calls a model, and nothing reaches a model until Lua sends it. To send prose to a model, declare a model role in the frontmatter, select it, and call `models.infer`:

````markdown
---
name: hello
description: Say hello
promptforge: 0
models:
  writer: {}
---

# Hello World

```lua
models.default("writer")
```

## Greet

Say "Hello, world!"

```lua
return models.infer(prose)
```
````

Each model piece is introduced here and taught in full later:

- `models:` with `writer: {}` declares a model role with the role label `writer` ([model roles at a glance](10-models.md#model-roles-at-a-glance)).
- `models.default("writer")` in a `lua` fence in the H1 body selects that role for the whole prompt, and `models.use('writer')` inside a section selects it for that section instead ([choosing a section's model](10-models.md#choosing-a-sections-model)). The H1 fence runs in [the H1 pass](04-how-a-prompt-runs.md#the-h1-pass), before `## Greet`.
- `models.infer(prose)` sends the section's prose to the model in one round and returns the reply text, and `return` makes that reply the result ([running a round with models.infer](10-models.md#running-a-round-with-modelsinfer)).

The run result of this prompt is whatever the model replies.

For a conversation instead of a single round, build a message list and run it with `models.loop`:

````markdown
---
name: first-chat
description: Holds a one-message conversation
promptforge: 0
models:
  writer: {}
---

# First Chat

## Talk

```lua
models.use('writer')
local msgs = messages.new()
msgs:user('hello')
models.loop(msgs)
return msgs[#msgs].content
```
````

`messages.new()` makes an empty message list, `msgs:user('hello')` adds a user record, and `models.loop(msgs)` runs the conversation under the selected role and appends the reply as the last record, which the block reads as `msgs[#msgs].content` because `models.loop` itself returns nil ([a first conversation](11-conversations.md#a-first-conversation)).

A model call needs a selected model role. This prompt sends its prose to a model without selecting one:

````markdown
---
name: no-role
description: Calls a model without selecting a role
promptforge: 0
---

# No Role

## Only

Ask the model.

```lua
return models.infer(prose)
```
````

It fails with the run error kind `Binding` ([how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)) and the message `model binding required for section {name}`, where `{name}` is the section's heading text:

````text
model binding required for section Only
````

If the block catches this failure with `pcall`, the error value it receives has the error kind `internal` ([catching and inspecting errors](05-lua-environment.md#catching-and-inspecting-errors)).

## How sections fit together

A prompt can hold several `##` sections, each with its own `lua` fence. They run in document order, starting at the first `##` section, the entry section, and each section whose Lua does not return falls through to the next, so a later section sees what an earlier one left behind. This walk from section to section is taught in [the section walk](04-how-a-prompt-runs.md#the-section-walk).

````markdown
---
name: handoff
description: Passes a note from one section to the next
promptforge: 0
---

# Handoff

## Writer

```lua
store.write('note.txt', 'handoff text')
```

## Reader

```lua
return store.read('note.txt')
```
````

````text
handoff text
````

`store.write` writes a file to the run's store, and `store.read` returns its text ([writing and reading files](09-the-store.md#writing-and-reading-files)). `## Writer` does not return, so it falls through, and `## Reader` returns what `## Writer` wrote.

The `var` table passes values along the walk the same way:

````markdown
---
name: three-steps
description: Builds a value across three sections
promptforge: 0
---

# Three Steps

## A

```lua
var.from_a = 'a'
```

## B

```lua
assert(var.from_a == 'a')
var.from_b = 'b'
```

## C

```lua
return var.from_a .. var.from_b
```
````

````text
ab
````

Three rules govern the walk:

- A section whose Lua does not return falls through to the next section.
- An explicit `return` stops the walk, and any later sections are never reached.
- When no section returns, the run result is `done` ([block and section returns](04-how-a-prompt-runs.md#block-and-section-returns)).

Sections can also run only when another section starts them. Here the entry section `## Main` starts its two siblings as tasks and waits for both before it returns:

````markdown
---
name: two-tasks
description: Starts two sections as tasks and joins their results
promptforge: 0
---

# Two Tasks

## Main

```lua
local a = tasks.spawn('## Alpha')
local b = tasks.spawn('## Beta')
local results = tasks.when_all({ a, b })
return results[1].result .. ' and ' .. results[2].result
```

## Alpha

```lua
return 'alpha'
```

## Beta

```lua
return 'beta'
```
````

````text
alpha and beta
````

The string `'## Alpha'` is a heading reference, which names a section by its level and heading text ([referring to a section by heading](02-file-structure.md#referring-to-a-section-by-heading)). `tasks.spawn('## Alpha')` starts that section as a task and returns a Task handle, and `tasks.when_all` waits for every handle and returns one entry per handle, in the order given, each with its `result` ([tasks at a glance](15-tasks.md#tasks-at-a-glance)).

`## Main` returns after the wait, and that return ends the walk, so the walk never falls through into `## Alpha` or `## Beta`. Each of them runs only as a task that `## Main` started, and its return goes back to `## Main`. Only the entry section's return becomes the run result.

Sections nest too. A `###` section under a `##` section is a child section with its own `lua` fence, all under the one H1 title ([sections and nesting](02-file-structure.md#sections-and-nesting)). The walk never enters a child section on its own: a child section runs only when addressed by heading, for example as the worker section of `fanout`:

````markdown
---
name: each-member
description: Runs a nested worker once per member
promptforge: 0
---

# Each Member

## Parent

```lua
local r = fanout('### Worker', {'a', 'b', 'c'})
return r[1].text .. '|' .. r[2].text .. '|' .. r[3].text
```

### Worker

```lua
return item
```
````

````text
a|b|c
````

`fanout('### Worker', {'a', 'b', 'c'})` runs `### Worker` once per member of the collection and returns one result per member in collection order, with the first one's text read as `r[1].text` ([the fanout call](14-fanout.md#the-fanout-call)). Inside each run of the worker, the member is available as `item` ([inside an arm](14-fanout.md#inside-an-arm)). `## Parent`'s return is the run result. A `jump` to a direct child also runs it, as a [child walk](08-jump-and-call.md#child-level-walks).

Without such a call, the walk goes from one `##` section straight to the next and skips the child sections in between:

````markdown
---
name: nested-child
description: Shows that the walk never enters a child section
promptforge: 0
---

# Nested Child

## A

```lua
var.seen = 'A'
```

### Child

```lua
error('a child must not run by fall-through')
```

## B

```lua
return var.seen .. 'B'
```
````

````text
AB
````

The walk runs `## A`, then `## B`. `### Child` never runs, so its `error` never fires, and the run succeeds.

One prompt can combine all of these parts: a shared library, prose in the H1 body, several sections, and a nested section of prose:

````markdown
---
name: tidy-subject
description: Lowercases a subject with a shared helper
promptforge: 0
---

# Tidy Subject

```lua shared
function normalize(value)
    return string.lower(value)
end
```

The shared helper is available to each executable section.

## Prepare

```lua
var.subject = normalize('Hello World')
```

### Author note

A note for people reading the file.

## Finish

```lua
return var.subject
```
````

````text
hello world
````

This file has a shared library, two sections, and one child section. The `lua shared` fence in the H1 body holds the shared library, code that loads in every section so each one can call its helpers ([the shared library](03-blocks-and-prose.md#the-shared-library)), which is why `## Prepare` can call `normalize`. No block reads the line of prose in the H1 body, so it is discarded. `var` passes the tidied subject from `## Prepare` to `## Finish`, and the walk never enters `### Author note`.

## The prompt and its host

Four optional frontmatter keys are the contract keys, the prompt's contract with the host:

- `capabilities:` names the capabilities the prompt needs from the host ([declaring capabilities](12-tools.md#declaring-capabilities)).
- `tools:` binds tool slots, each an alias mapped to a tool path ([tool slots and Tool objects](12-tools.md#tool-slots-and-tool-objects)).
- `models:` declares model roles ([declaring roles](10-models.md#declaring-roles)).
- `args:` types the prompt's arguments ([arg declarations](06-arguments.md#arg-declarations)).

The parser checks only the shape of these keys. The host satisfies them at prepare, the step before the run starts ([what a run does](04-how-a-prompt-runs.md#what-a-run-does)). A declaration the host cannot satisfy fails the run with the run error kind `RequirementsUnmet` before any section runs ([how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)), and prepare's refusal text is the requirements notice ([when a run cannot start](04-how-a-prompt-runs.md#when-a-run-cannot-start)).

A prompt that declares no capabilities, tools, or model roles has no requirements for the host to meet before the run. It runs on its Lua sections alone, with no model, no tools, and no host files behind it:

````markdown
---
name: no-requirements
description: Runs on Lua alone
promptforge: 0
---

# No Requirements

## Only

```lua
return 'no capabilities'
```
````

````text
no capabilities
````

When a prompt does declare model roles and tool slots, the frontmatter decides what is bound. Before the run begins, the host fills every declared model role with a concrete model and every tool slot with the tool at its declared tool path ([filling tool slots and model roles](04-how-a-prompt-runs.md#filling-tool-slots-and-model-roles)). For the `writer: {}` role in the prompts above, prepare fills `writer` with the host's current model and reports the declaration satisfied, and a section's `models.use('writer')` then runs its rounds under that model.

Lua only ever selects labels that are already bound:

- `models.use` selects only a role label that the frontmatter declared and the host filled, and raises `models.use label "{label}" is not a bound model role` for any other label.
- `tools.add` puts a slot's tool in scope for the model in the current section ([advertising tools to the model](12-tools.md#advertising-tools-to-the-model)), accepts only an alias whose slot is filled, and fails with `tools.add alias "{alias}" is not a bound tool slot` for any other alias.

The model calls a bound tool by its alias from `tools:`, such as `search`, never by its tool path, such as `promptforge/web/search`.

The language itself performs no I/O and reads no clock. A prompt reaches the outside world only through host work: requests the host performs for the run and answers. There are exactly six kinds of host work:

- A model round: `models.infer` sends one round with its prompt text and no tools, and `models.loop` runs rounds ([running a round with models.infer](10-models.md#running-a-round-with-modelsinfer)).
- A bound tool call ([calling tools from Lua](12-tools.md#calling-tools-from-lua)).
- A wait for operator input ([asking the operator with user_input](05-lua-environment.md#asking-the-operator-with-user_input)).
- A store operation, one for each store call ([what the store is](09-the-store.md#what-the-store-is)).
- The timer behind a timed wait on tasks ([time limits on waits](15-tasks.md#time-limits-on-waits)).
- A read of a task's event history ([reading a task's history](16-task-events.md#reading-a-tasks-history)).

Everything else a block does, such as computing and returning a value, is plain Lua that needs no host work, which is why a literal `return` asks the host for nothing. While the host performs host work, only the chain that asked for it waits ([waiting and reproducibility](04-how-a-prompt-runs.md#waiting-and-reproducibility)).

This section asks for two store operations, a write and then a read:

````markdown
---
name: notes
description: keeps a note
promptforge: 0
---

# Notes

## Save

```lua
store.write('todo.md', 'ship it')
return store.read('todo.md')
```
````

````text
ship it
````

All four contract keys can sit together in one frontmatter block, alongside `name`, `description`, and `promptforge: 0`:

````yaml
---
name: research
description: Searches public and private sources
promptforge: 0
capabilities:
  - promptforge/web
  - ref: io.github.corp/mcp
    optional: true
    config: { servers: [alpha] }
tools:
  search: promptforge/web/search
  fetch: promptforge/web/fetch
args:
  use_mcp: { type: boolean, default: true, description: Search MCP-connected private sources }
models:
  analyst: { keywords: [frontier, thinking], min_context: 200000, description: deep reasoning }
  triage: { keywords: [fast, small], description: quick triage of search results }
---
````

- `capabilities:` lists `promptforge/web` as a required capability id ([the web capability](13-web-fetch-and-search.md#the-web-capability)), then a mapping that names `io.github.corp/mcp` with `ref:`, marks it `optional: true`, and gives it its own `config:`.
- `tools:` maps two aliases, `search` and `fetch`, to tool paths, and the model calls the tools as `search` and `fetch`.
- `args:` declares `use_mcp`, a `boolean` argument with a default of `true` and a description.
- `models:` declares two roles, `analyst` and `triage`, each with `keywords` and a `description`, and `analyst` also sets `min_context`.

A prompt with this frontmatter still needs its H1 title and the H1 body blocks or sections that do its work, and prepare checks each declaration against the host before the run starts.
