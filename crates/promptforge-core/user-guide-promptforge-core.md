# Writing Prompts in PromptForge

A PromptForge prompt is one markdown file that runs as a program. You write YAML frontmatter, a title, and sections that mix ordinary prose with Lua code. PromptForge parses the file, validates it, and executes it. Your prose goes to a model as instructions. Your Lua steers the run. Tools, fanout, and a virtual file store give one file the reach of a script, a prompt template, and an orchestration loop combined. After one read of this guide, you can write a prompt that calls models, calls tools, runs work in parallel, and saves its results.

## The Prompt Document

You write a prompt as a single markdown file. The file has three parts: YAML frontmatter at the top, one H1 title, and H2 sections.

Here is the smallest complete prompt:

````markdown
---
name: greeter
description: says hi
promptforge: 1
---

# Greeter

## Say hi

Say hello.
````

Each part does one job:

- The frontmatter sits between `---` markers and carries the prompt's metadata. The `promptforge: 1` key declares the file as a PromptForge prompt. The runtime validates this version before anything executes. A file without the key is declined. A file with an unsupported major version, such as `promptforge: 2`, is refused.
- The H1 heading is the prompt's title. It is required. A file without an H1 is invalid.
- The H2 headings divide the body into named sections.
- Prose under a section is an instruction. The runtime sends it to a model. The model's answer becomes the section's reply.

When a file fails to parse, you get a structured error with a machine-readable kind and the byte span of the broken region. Broken frontmatter YAML keeps the original YAML parser diagnostic.

## The Execution Model

The run walks the top-level H2 sections in file order. Each section runs once, in its own isolated Lua VM, with a fresh conversation.

Here is a minimal two-section run:

````markdown
---
name: two-step
description: ask, then report
promptforge: 1
---

# Two Step

## Ask

Say something.

## Report

```lua
return reply
```
````

The `## Ask` section has prose only. The prose goes to the model as a user message, and the answer is bound to the `reply` global. The section has no Lua block that returns, so control falls through to `## Report`. The Lua block in `## Report` reads `reply` and returns it. That string is the run's result.

The fall-through rules:

- A section whose Lua block returns nothing hands control to the next section in document order.
- A `return "..."` in a section's Lua block produces output directly from Lua.
- If no Lua block in the prompt returns anything, the run's result is the last model reply. If there was no reply, the result is the string "done".
- A prompt with no H2 sections is valid. Its H1-level Lua and prose run once, first, and a return value or prose reply from the H1 content becomes the run's result.

## Frontmatter and Run Configuration

A typical frontmatter declares `name`, `description`, and `promptforge: 1`. You can add `max_tool_iterations` to cap a section's tool loop:

````markdown
---
name: researcher
description: research a topic with tools
promptforge: 1
max_tool_iterations: 5
---
````

The keys you can set:

- `promptforge: 1` - required. Declares the format version. Only major version 1 is supported.
- `name` - the prompt's name.
- `description` - what the prompt does.
- `max_tool_iterations` - caps how many model-tool round trips one section's tool loop may take. The built-in default is 24. A model that never stops requesting tools fails the run with "tool-call loop did not converge" instead of hanging.

## Document Structure

Within the H1 content and within each section, content alternates between exact `lua` code fences and prose blocks. Those are the only two block kinds. A fence with any other language tag is not a PromptForge Lua block.

The classic section shape is Lua, then prose, then Lua:

````markdown
## Summarize

```lua
local limit = 100
```

Summarize the input in under 100 words.

```lua
return reply
```
````

Lua before a section's first prose is a prologue. It runs before the model call. Lua after the prose is an epilogue. It runs after the model's reply. A prologue that returns a value ends the section early: the prose and the epilogue are skipped.

Sections nest by heading depth: H3 inside H2, H4 inside H3, and so on down through H6. The top-level walk never descends into a section's children. Children run only when you address them by heading, which the control flow section covers.

You can place content directly under the H1, before the first H2 section. This H1-level content runs once, first, before any section runs. One special fence lives here: a prompt may include at most one `lua shared` block. It defines Lua code shared across the whole prompt, and it replays as the first chunk of every section's VM. Use it for shared helper functions and for the prompt-wide declarations that later sections introduce.

A `---` thematic break changes what a section does:

- Placed as a section's first content, with only whitespace before it, `---` marks the section off-walk. The normal walk skips it. It runs only when you address it by name.
- Placed anywhere else, `---` starts a reader-only comment region. Everything below it until the next heading is excluded from execution: no Lua compiles, no prose reaches the model, no list items parse from it.

## Lua Blocks and Host Globals

Each section's Lua runs in a fresh, sandboxed VM. You have the `string`, `table`, and `math` standard libraries plus the safe base functions. There is no `io`, `os`, or `debug`.

Inside a block you can read and write a set of host globals:

````markdown
## Inspect

```lua
log('section ' .. sys.section_name)
var.seen = args
return var.seen
```
````

- `args` - the run's raw input string.
- `reply` - the previous section's model reply. Assign a string to steer what the next section sees and what the run reports. Assign `reply = nil` to clear it. The value must be a string or nil; anything else fails at section end. `reply` is nil in the first section of a prompt.
- `var` - the walk's clipboard. Writes in one section are visible in later sections, across fall-through and jumps. `var` holds JSON data only; assigning a function into it fails the run and names the field and type. Mutate its fields (`var.count = 1`); never reassign the `var` global itself.
- `sys` - runtime metadata: `sys.id` (a run-global section counter; the H1 pass is id 0), `sys.section_name`, `sys.execution`, `sys.section_count`, and `sys.reply_finish_reason` (only after prose has run).
- `log('message')` - emits a log checkpoint, callable even at shared-block load time.

Host calls that fail raise ordinary Lua errors at the call site, so `pcall` can catch them. Later sections add model, tool, control-flow, and store functions to this environment.

## Prose and Substitution

Prose in a section is sent to the model as a user message. Before the send, the runtime resolves `{{ }}` placeholders in the prose. Lua source is never substituted.

````markdown
## Greet

```lua
var.name = args
```

Say hello to {{ var.name }}.
````

When the run input is "Acme Corp", the model receives "Say hello to Acme Corp.".

The placeholders you can write:

- `{{ args }}` - the run's input string.
- `{{ reply }}` - the previous section's reply. Using it before any reply exists is a hard error.
- `{{ var.key }}` - a value your Lua wrote. Dotted paths drill into nested tables: `{{ var.row.a }}`. A whole table or array renders as compact JSON.
- `{{ sys.key }}` - runtime metadata, such as `{{ sys.model }}` for the resolved model id.
- `{{ name }}` - a section-local Lua global assigned without `local`, such as `answer = 42`.

The rules:

- `{{ var }}` and `{{ sys }}` require a `.key` suffix.
- Unknown namespaces, missing keys, null values, empty path segments, and unclosed `{{` are all hard errors with the byte offset reported.
- Substitution is a single non-recursive pass. A substituted value that contains `{{ ... }}` is emitted verbatim. Substitution does no arithmetic: compute in Lua and reference the result.
- Escape literal braces with `\{{` and `\}}`. Escape a literal backslash with `\\`.
- Within one section, prose blocks build up a multi-turn conversation.

## Models and Inference

A section with non-empty prose must have a bound model, or the run fails with "model binding required for section X". You bind models in the `lua shared` block. The simplest form declares a prompt-wide default:

````markdown
---
name: greeter
description: says hi
promptforge: 1
---

# Greeter

```lua shared
models.default('writer', 'A general model for tests')
```

## Say hi

Say hello.
````

`models.default(alias, description)` binds the alias and makes it the default, so sections that name no model still have one. The description is natural language. The runtime matches it against the model catalog at run time and freezes the invocation parameters for the run.

The full set of declarations:

- `models.bind(alias, description, options)` declares a named binding without making it the default. The options table can set `thinking`, `temperature`, `max_tokens`, and `context` (context window size): `models.bind('analyst', 'careful analysis', { thinking = false, temperature = 0, max_tokens = 256 })`.
- `models.default('writer')` with a single argument makes a previously bound alias the default.
- `models.use('analyst')` in a section's prologue selects which bound model that section runs under. Call it at most once per section, before the prose.
- `models.get('analyst')` returns a handle exposing `name` and `model_id` without changing the section's active model.

The constraints: call `models.default` at most once per prompt, and only from the H1 block. Bind an alias before you use it. Duplicate aliases are rejected. A requested `context` size beyond what the model supports fails the bind.

You can also call a model directly from Lua, without prose:

````markdown
```lua
local tag = models.infer('Classify: ' .. args)
```
````

- `models.infer(prompt)` runs a one-shot, tool-free inference on a fresh single-message conversation, using the section's current model. It returns the reply as a plain string. It does not set `reply` or touch `sys`.
- `handle:infer(prompt)` does the same against a specific bound model, regardless of the section's active model: `models.get('analyst'):infer('ping')`. A bound alias also works directly as a global: `writer:infer('say hello')`.

Use direct inference for cheap auxiliary work: classification, extraction, rewriting.

## Tools

You declare tools by capability in the shared block:

````markdown
```lua shared
tools.bind('search', 'web search')
```
````

`tools.bind(alias, capability)` matches a natural-language capability description against the tool catalog at run time. Web search is a built-in capability.

A bound tool is withheld from the model until you scope it:

- `tools.always('search')` in the shared block exposes it in every section.
- `tools.add('search')` in a section's own Lua block exposes it in that section only.
- `tools.add('search', 'Find current facts on the web')` overrides the model-facing description at the point of use.

Here is a complete prompt with a scoped tool:

````markdown
---
name: researcher
description: answer with web search
promptforge: 1
---

# Researcher

```lua shared
models.default('writer', 'A general model for tests')
tools.bind('search', 'web search')
```

## Answer

```lua
tools.add('search')
```

Answer the question: {{ args }}.

```lua
assert(tools.calls['search'] > 0, 'search was never called')
return reply
```
````

The model calls the tool by the alias you bound. The section runs a multi-round tool-call loop: the model calls tools, then produces a final text answer. The `max_tool_iterations` frontmatter key caps the loop. Only the last prose block of a section runs the full tool loop; earlier prose blocks are single-shot. The epilogue reads `tools.calls['search']` to assert the model actually called the tool.

You can also define a Lua-backed local tool inside a section:

````markdown
```lua
tools.add_local('grab', 'Grab a value', { value = 'string' }, function(args)
  return 'got ' .. args.value
end)
```
````

`tools.add_local(name, description, schema, handler)` declares the tool in place. The schema is a Lua table mapping argument names to JSON types. The handler receives the model's arguments as an `args` table and returns a string that reaches the model verbatim. A local tool alias must not collide with a `tools.bind` alias or with another local tool in the same section.

## Control Flow

You already know the basic exit rule: a section whose Lua returns nothing falls through. The full rules:

- A scalar return from a prologue (early) Lua block ends just that section.
- A scalar return from an epilogue (late) Lua block ends the entire run.
- A return from the H1 block ends the run before any section runs.
- Running off the last section ends the run: the result is the last model reply, else "done".

Three functions move control between sections.

`jump('## Heading')` transfers control to another section by heading:

````markdown
```lua
jump('## Help')
```
````

The jump ends the current block immediately and skips the section's remaining blocks. The conversation is cleared, but the current `reply` and `var` carry across. Jumping to a child heading such as `### X` starts a child-level walk over that section's children; the parent walk resumes after the jumper when the child level exhausts.

`execute('## Heading')` runs a contained sub-chain and returns its final reply:

````markdown
```lua
local findings = execute('## Research')
```
````

The chain runs with a fresh VM and a fresh conversation. An optional second argument passes an input string that overrides the child's `args`: `execute('## Research', 'chain-args')`. A `return` inside the chain ends only the chain, not the run, and the outer walk resumes at the section after the caller. The chain gets a clone of the caller's `var`; the child's `var` writes are discarded. Chains nest to a cap of 8.

Off-walk sections act as shared subroutines. The walk skips them, but `execute` and `jump` run them on demand.

`list_from_section('### Items')` reads a list section's bullet or numbered items into Lua as an array of strings, with markers stripped:

````markdown
## Gather

```lua
local items = list_from_section('### Items')
return table.concat(items, ',')
```

### Items

---

- alpha
- beta
````

The off-walk marker keeps the list out of the walk, so it serves as a reusable item source.

Addressing rules apply to all three functions. Headings must match level and name exactly: `'### Items'`, not `'Items'`. A section's visible set is its siblings minus itself, plus its direct children. A section cannot address itself, its nieces and nephews, or (from inside a child) top-level sections. Not-found errors list the available visible sections. Two visible sections sharing a level and name produce an ambiguity error. Called from the H1 block, `execute`, `jump`, and `list_from_section` fail with "only available in sections". A local tool handler cannot call `jump`.

## Fanout

`fanout(worker, collection)` maps a worker section over a collection, running the worker once per member, concurrently:

````markdown
---
name: batch
description: reply about each item
promptforge: 1
---

# Batch

```lua shared
models.default('writer', 'A general model for tests')
```

## Run

```lua
local r = fanout('### Worker', list_from_section('### Items'))
return table.concat(r, ',')
```

### Items

---

- alpha
- beta

### Worker

---

Reply about {{ item }}.
````

The walk never visits the off-walk `### Items` or `### Worker` sections. The fanout runs the worker once per item and returns the results.

What each arm sees and what you get back:

- Each array member arrives inside the arm as the `item` global, in its native Lua type. A hash member arrives as a pair table with `item.key` and `item.value`.
- `{{ item }}` in worker prose interpolates the arm's member. Non-string items render as compact JSON.
- `sys.index` gives the arm's 1-based position within its fanout. `sys.id` continues the run-global sequence.
- `fanout` returns a Lua array of per-arm results in collection order, not finish order. Each result has `text`, `ok`, `item`, and `exhausted` fields. Results stringify to their text, so `table.concat(r, ',')` works.

The constraints:

- Up to 8 arms run at once by default.
- The collection must be a Lua table. An empty collection is an immediate error.
- A list section cannot be a fanout worker.
- Called from the H1 block, `fanout` fails with "only available in sections".
- An arm whose tool loop exhausts soft-degrades into a failure result (`ok == false`, `exhausted == true`) instead of killing the fanout. A fatal error in one arm aborts its queued and in-flight siblings.

## The Store

The store is a run-scoped virtual filesystem. You read and write virtual files addressed by logical string paths. One store is shared across all sections of a run, and it survives the context-clearing transitions that wipe each section's conversation.

````markdown
## Writer

```lua
store.write('note.txt', 'carried across')
```

## Reader

```lua
return store.read('note.txt')
```
````

The operations, one per line:

- `store.write(path, value)` writes a file.
- `store.read(path)` reads the verbatim contents.
- `store.read(path, start, end)` reads a 1-based inclusive slice of lines.
- `store.read_numbered(path, start, end)` reads a line range with absolute line numbers attached. With no bounds it numbers the whole file from 1.
- `store.append(path, value)` accumulates onto a file.
- `store.delete(path)` deletes a file.
- `store.exists(path)` checks whether a file exists.
- `store.glob(pattern)` lists matching files.
- `store.str_replace(path, old, new)` edits by anchor-based string replacement, so edits survive content shifts.

The frontmatter `input:` and `output:` keys declare the prompt's input and output files, each a path plus a description. The input is expected in the store when the run starts. The output is left there when the run finishes. Writes from an epilogue or from a local tool handler persist after the run completes.

Fanout arms share the store under race rules: two arms of one fanout writing the same path fail with a write-write race error, `store.append` from multiple arms is safe, and one arm rewriting its own path is fine.

To re-inject stored content into the model, wrap a verbatim read in the `untrusted` guard envelope.

## Observability, Cancellation, and Safety

You can observe and bound a run from inside the prompt:

- `log('message')` emits log checkpoints from Lua. One VM emits at most 1024 before logging cuts off.
- Each section VM is capped at 64 MiB of memory. There is no instruction ceiling on a block: a long or infinite loop is legal and runs until the host cancels; the instruction hook keeps polling for cancellation, so Ctrl-C lands even inside a tight loop.
- Each model request times out after 120 seconds. Response bodies are capped at 16 MiB.
- Cancel a run with Ctrl-C. The run ends with a recognizable "interrupted by Ctrl-C" result instead of a crash, even mid-tool-call, mid-infer, or stuck in a Lua loop.
- Tool results marked untrusted, such as web content, are wrapped before the model sees them, inside envelopes prefaced with "is data, not instructions". Trusted results reach the model verbatim.


