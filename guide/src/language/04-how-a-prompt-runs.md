# How a Prompt Runs

Every run of a prompt follows one fixed order, so you can read a prompt top to bottom and know exactly what will happen: the host checks that it can give the prompt what it asks for, the H1 body runs once as setup, the sections run in file order, and the run ends with a result, a cancel, or a failure that names its cause. This chapter walks through that order step by step, so you can decide where each piece of a prompt belongs, return the result you want from the right place, stop a run early when its input is wrong, and read the notice the host gives when it cannot start your prompt at all.

## What a run does

A run is one execution of a prompt file. Every run goes through the same steps in the same order:

1. Prepare. Before any Lua runs, the host sets up everything the prompt's frontmatter asks for and checks that it can provide it. [Capability activation](#capability-activation) and [Filling tool slots and model roles](#filling-tool-slots-and-model-roles) describe this step.
2. The H1 pass. When the H1 body holds anything that runs, a `lua` block or prose, it runs once first, with the same access to the host as any section. [The H1 pass](#the-h1-pass) covers it. A prompt whose H1 body holds neither goes straight to its first section.
3. The walk. The top-level `##` sections run top to bottom in file order, starting from the first one, and each one falls through to the next when it finishes. [The section walk](#the-section-walk) covers it.
4. The outcome. The run ends completed with a result, cancelled, or failed.

Here is a prompt with two sections. The first section's Lua returns nothing, so it falls through, and the second section returns a value:

````markdown
---
name: two-sections
description: The first section falls through and the second returns
promptforge: 0
---

# Title

## First

```lua
local x = 1
```

## Second

```lua
return "second"
```
````

Its run result:

````text
second
````

All of `## First` runs, then all of `## Second`. Each section finishes before the next one starts, and a section whose Lua does not return anything falls through to the next sibling section at the same level, unless a section ends the run early ([Block and section returns](#block-and-section-returns)) or moves the walk somewhere else ([The section walk](#the-section-walk)).

A completed run gives back its result as a single text string. A `return` in the Lua block of a section on the walk makes the returned value the run's result. In a prompt with several sections, that is usually the last section: earlier sections that return nothing hand off to the next one, and a section that calls a model without returning the reply adds nothing to the result. A common last line is `return models.infer(prose)`, the call [Prose and Lua](01-what-a-prompt-is.md#prose-and-lua) introduced, which makes the model's reply the run's result.

A return from a section on the walk ends the whole run at once. The returned value becomes the result, and the rest of that section and every later section never run, even sections the walk would otherwise reach:

````markdown
---
name: first-wins
description: The first section's return ends the run
promptforge: 0
---

# Title

## First

```lua
return "first"
```

## Second

```lua
return "unreached"
```
````

Its run result:

````text
first
````

When a run finishes without any block returning a value, for example when the walk runs past the end of the last top-level section, the result is the fixed text `done`. This is the `done` fallback:

````markdown
---
name: no-return
description: One section that returns nothing
promptforge: 0
---

# Title

## Only

```lua
local x = 1
```
````

Its run result:

````text
done
````

Every run ends in exactly one of three outcomes:

- Completed, with its result text.
- Cancelled, reported as its own outcome and not as a failure.
- Failed, with a run error kind that names the cause, such as an uncaught Lua error, and a message for people to read.

This chapter's features can fail a run with two run error kinds: `Lua` for an uncaught Lua error in a section, and `RequirementsUnmet` when the host cannot satisfy the prompt or the H1 pass fails. [How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified) lists every run error kind, and [Failure and cancellation](#failure-and-cancellation) shows how a run fails or is cancelled.

A run can also be refused before it starts. When the host cannot provide what the prompt requires, for example a model with a large enough context for one of the prompt's roles, no section runs and no Lua runs, so nothing in the prompt can catch the refusal. The run fails with run error kind `RequirementsUnmet`, and the error's message is the requirements notice, which lists what is missing:

````text
the environment cannot satisfy this prompt:
- role 'analyst': requires a context of at least 200000 tokens; the current model provides 32000
````

[When a run cannot start](#when-a-run-cannot-start) gives the full layout of the requirements notice.

## The section walk

The walk is how the run moves through sections: it enters one section, runs it, and falls through to the next sibling at the same level. The walk the run itself starts, over the top-level `##` sections, is the main walk.

The main walk starts at the entry section, which is the first top-level `##` section in file order, whatever its name. With `## Zebra` written before `## Main`, the walk starts at `Zebra`, so you choose the starting section by placing it first. A prompt with no `##` section has no entry section; its [H1 pass](#the-h1-pass), if it has one, is the whole run.

Inside a section, the blocks run top to bottom, as [Section shapes](03-blocks-and-prose.md#section-shapes) showed. After the last block, the section falls through: it ends with no result, and the walk moves on to whatever follows.

Each time the walk enters a section, that section entry starts fresh, with its own [section VM](03-blocks-and-prose.md#how-the-shared-library-loads), the Lua instance its blocks run in, and its own state. When the walk falls through, the old section VM is torn down and the next section entry builds its own.

The main walk is a chain: a walk over sibling sections that something begins, here the run itself. Later chapters show calls that begin chains of their own, and some rules below depend on which chain a section runs in.

Fall-through never goes down into child sections. The walk moves only between siblings at one level, so a `###` child under `## A` is passed over, and the walk goes from `## A` straight to `## B`:

````markdown
---
name: children-wait
description: The walk moves from sibling to sibling and skips the child
promptforge: 0
---

# Title

## A

```lua
local x = 1
```

### Child

```lua
error('a child must not run by fall-through')
```

## B

```lua
return 'B'
```
````

Its run result:

````text
B
````

The child's `error` never fires, and the run succeeds. A child section runs only when a call names it by [heading reference](02-file-structure.md#referring-to-a-section-by-heading), such as `jump('### Child')` from its parent, which starts a walk over the children, or `call('### Child')`, which runs it and comes back ([Jump and call at a glance](08-jump-and-call.md#jump-and-call-at-a-glance), [Child-level walks](08-jump-and-call.md#child-level-walks)).

Every top-level section is on the main walk; no syntax takes a section out of it. A top-level section is skipped only when an earlier section ends the run with a return or moves the walk past it with `jump`.

## Block and section returns

A `return` at the top level of a Lua block ends that block, and the returned value is the block's result. What happens next depends on what you return. This is the scalar return rule:

- A string, integer, float, or boolean becomes the result, as text.
- Nothing, or `nil`, gives no result, and the run keeps going.
- Anything else fails the block with `cannot return a {type} as a result`.

A block that returns nothing or `nil` keeps the run going: the next block in the same section runs, then the next section, where a later `return` can set the result. Having no result is different from returning an empty string, which is a result.

Only the first returned value counts, and the values after it are never checked. Each kind of value becomes text like this:

| You write | The result |
|---|---|
| `return 'text'` | `text` |
| `return 42` | `42` |
| `return 1.5` | `1.5` |
| `return 3.0` | `3` |
| `return true` | `true` |
| `return 'a', 'b'` | `a` |
| `return nil`, or no `return` | no result; the run keeps going |
| `return {}` | fails with `cannot return a table as a result` |

A string comes back as is, except that invalid UTF-8 bytes become U+FFFD. An integer becomes its decimal digits. A float is written in plain display form, never in exponent form, and infinity and NaN become `inf` and `NaN`. A boolean becomes `true` or `false`.

In the failure message, `{type}` is the Lua type name: `table`, `function`, `userdata`, or `thread`. It is an ordinary Lua error, so in a walked section it fails the run with run error kind `Lua`. To return data held in a table, build a string from it first, for example with `table.concat`.

Returning `prose` makes the block's rendered [pending prose](03-blocks-and-prose.md#the-pending-prose-buffer) the result:

````markdown
---
name: echo-prose
description: Returns the prose written above its block
promptforge: 0
---

# Echo Prose

## Main

Say hello to the reader.

```lua
return prose
```
````

Its run result:

````text
Say hello to the reader.
````

A value returned from a section's prologue, its leading `lua` block ([Lua blocks and prose blocks](03-blocks-and-prose.md#lua-blocks-and-prose-blocks)), ends the section at once with that value as the result. The prose after the prologue is never sent to a model, the epilog after the prose never runs, and no model is needed at all:

````markdown
---
name: stop-early
description: A prologue return skips the prose and the epilog
promptforge: 0
---

# Title

## Stop Early

```lua
return 'early'
```

This prose never reaches a model.

```lua
return 'late'
```
````

Its run result:

````text
early
````

A section that returns partway through still finishes normally, exactly as when it falls through: it is reported as finished, and its final `var`, the table that keeps values between blocks and sections ([Keeping values in var](05-lua-environment.md#keeping-values-in-var)), is read back.

A scalar return ends the chain it runs in, and the value becomes that chain's result. On the main walk, that means a return from any section, including one the walk reached through `jump`, ends the whole run with that value as the run's result. A section can also run in a chain of its own, and there its return goes back to whoever started it: in a called chain it becomes the value `call` returns ([Called chains](08-jump-and-call.md#called-chains)), in a fanout arm it becomes that arm's result ([Results](14-fanout.md#results)), and in a task it becomes the task's result ([Waiting for results](15-tasks.md#waiting-for-results)). Those returns never end the run; the run's result still comes from a return in the H1 pass or on the main walk, or from the `done` fallback.

## Failure and cancellation

To stop a run on purpose, call `error('message')` in a section's Lua block. The run fails with run error kind `Lua`, and the rest of the prompt never runs:

````markdown
---
name: second-fails
description: The second section fails the run on purpose
promptforge: 0
---

# Title

## First

```lua
local x = 1
```

## Second

```lua
error('expected failure')
```
````

Its outcome is failed, with run error kind `Lua` and an error message that contains `expected failure`. `## First` finishes normally. `## Second` is never reported as finished; its section VM is still torn down, exactly once, and then the run is reported as failed. Any other uncaught Lua error in a walked section, such as `assert(false, 'message')` or a call to a function that does not exist, ends the run the same way. The same failure in a block in the H1 body ends the run as `RequirementsUnmet` instead, as [The H1 pass](#the-h1-pass) explains.

A failed run always comes with a run error kind that classifies the cause by condition, together with a message for people to read. [How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified) lists them all.

When the host cancels a run, the run ends as cancelled. A cancel is its own outcome and never a failure. It stops the run even while its Lua is busy in a loop or while a block waits on a model's reply, and a model request cut short by a cancel is not reported as a failed model call. A cancel that lands while every chain is waiting takes effect at the next step.

The first outcome that ends a run is the one reported. Once a run has failed, completed, or been cancelled, a later error or cancel never replaces that outcome.

## The H1 pass

The [H1 body](02-file-structure.md#the-h1-title-and-its-content) is everything between the H1 title and the first `##` section. When it holds a `lua` block or prose, it runs once as the H1 pass, before the walk starts, as [Blocks under the H1](03-blocks-and-prose.md#blocks-under-the-h1) promised. Prose alone under the title also starts the pass. A `lua shared` fence alone does not: a prompt whose H1 body holds only its [shared library](03-blocks-and-prose.md#the-shared-library), or nothing at all, starts directly at its first section, with no extra pass.

The H1 pass hands state to the walk through `var`, the table that keeps values from block to block ([Keeping values in var](05-lua-environment.md#keeping-values-in-var)). The pass starts with an empty `var`, its final `var` is read back when it ends, and the main walk starts from that `var`:

````markdown
---
name: seed-the-walk
description: The H1 pass seeds var for the first section
promptforge: 0
---

# Seed the Walk

```lua
var.from_h1 = 'seed'
```

## A

```lua
return var.from_h1
```
````

Its run result:

````text
seed
````

A `lua` fence in the H1 body is the usual place for prompt-wide setup. What setup calls set is shared by the whole run, so every later section sees it. `models.default('writer')` picks the model role that later model calls use when they name none, so a section can then call `models.infer(prose)` without naming a model ([Choosing a section's model](10-models.md#choosing-a-sections-model)). `tools.always` offers tools to the model in every section ([Advertising tools to the model](12-tools.md#advertising-tools-to-the-model)). Here is the setup in place:

````markdown
---
name: greet
description: Greets the reader through the default model
promptforge: 0
models:
  writer: {}
---

# Greet

```lua
models.default('writer')
```

## Say Hello

Say hello to the reader in one sentence.

```lua
return models.infer(prose)
```
````

Its run result is the model's reply.

Lua in the H1 body works like Lua in any section. The H1 pass gets the same globals, the same shared library, and every host call a section can make, including model calls, [`list_from_section`](03-blocks-and-prose.md#reading-list-items-from-lua), and `jump`, `call`, and the other calls that move between sections ([Control from the H1 pass](08-jump-and-call.md#control-from-the-h1-pass)). It follows the walk's rules with three differences: it runs first, ahead of the main walk's sections; a scalar return from it ends the whole run; and an uncaught Lua error in it is the prompt's failed hard gate, described below.

The H1 body can hold several `lua` blocks separated by prose. They run in file order as one pass, each exactly once:

````markdown
---
name: count-blocks
description: Two blocks in the H1 body each run exactly once
promptforge: 0
---

# Count Blocks

```lua
var.executions = (var.executions or 0) + 1
```

Ask for one round.

```lua
var.executions = var.executions + 1
```

## Result

```lua
return var.executions
```
````

Its run result:

````text
2
````

A scalar returned from a block in the H1 body ends the whole run at once with that value as the run's result, so no `##` section runs:

````markdown
---
name: early-exit
description: An H1 return ends the run before any section
promptforge: 0
---

# Early Exit

```lua
return 'early'
```

## Never

```lua
error('the walk must not start after an H1 return')
```
````

Its run result:

````text
early
````

The final `var` is still read back on that early exit, and the H1 pass is never reported as a finished section.

When the H1 pass finishes with neither a scalar return nor a jump, it falls through, and the main walk starts at the first top-level section. When a block in the H1 body jumps, the main walk starts at the section the jump names instead.

A prompt with Lua in its H1 body but no `##` sections ends when the H1 pass ends. A scalar return in that Lua is the run's result, and without one the result is `done`:

````markdown
---
name: h1-only
description: An H1 pass with no sections
promptforge: 0
---

# Title

```lua
return "hello"
```
````

Its run result is `hello`. With `local x = 1` in place of the return, the result is `done`. A prompt with only an H1 title, nothing that runs under it, and no sections finishes at once with the result `done`.

### The hard gate

The H1 pass is the prompt's hard gate. When a block in the H1 body raises an ordinary Lua error that nothing catches, such as an `error` or `assert` call, a call to a function that does not exist, or running out of Lua memory, the run ends before any section is walked, with run error kind `RequirementsUnmet`, and the message is the Lua error text. Later blocks in the H1 body and every section never run. This makes the H1 body the place to check a prompt's input or setup with `assert`:

````markdown
---
name: gate
description: A failed H1 assertion stops the run before the walk
promptforge: 0
---

# Gate

```lua
assert(false, 'the gate cannot hold')
```

## Result

```lua
return 'unreachable'
```
````

Its outcome is failed, with run error kind `RequirementsUnmet` and a message that contains `the gate cannot hold`. `## Result` never runs. A call to a function that does not exist, an `error(...)` call, or any other ordinary Lua error in a block in the H1 body fails the run the same way. The same error anywhere else ends the run as `Lua` ([Failure and cancellation](#failure-and-cancellation)).

Only ordinary Lua errors from the H1 body's own blocks make up the gate. Failures around those blocks keep run error kind `Lua`: the shared library failing as it loads, a reassigned `var` found when the pass's `var` is read back, and a jump from the H1 pass whose heading matches no top-level section or more than one. Every other failure keeps its own run error kind, and a cancel keeps its cancelled outcome. [How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified) gives the full mapping.

### Names and order in the H1 pass

Errors and reports from the H1 pass name the prompt's title as their section, while those from a walked section name that section. Under `# Log Checkpoints`, a `log("shared loaded")` call in the H1 body records its checkpoint under `Log Checkpoints`, ahead of every section's checkpoints ([Checkpoints with log](05-lua-environment.md#checkpoints-with-log)).

Every part of a run follows the same fixed order. The H1 pass runs first: its section VM replays the shared library, runs the blocks in the H1 body, and is torn down. Then each `##` section on the walk runs in its own section VM: the section starts, the shared library replays, the prologue and the epilog run, the section VM is torn down, and the section finishes. The run succeeds after the last section. The replay step happens even when the prompt has no `lua shared` fence. The H1 pass reports no section start or finish of its own.

## Capability activation

Prepare begins with capability activation. A prompt's `capabilities:` key lists the capabilities it needs, each a set of tools the host provides, named by a capability id such as `promptforge/web`; a plain entry is required, and an entry written with `optional: true` is optional ([Declaring capabilities](12-tools.md#declaring-capabilities)):

````yaml
capabilities:
  - promptforge/web
````

````yaml
capabilities:
  - ref: promptforge/web
    optional: true
````

Each declared capability activates exactly once per run, before the rest of prepare, in declaration order, and the tools it contributes join the run's available tools in that same order.

A required capability must be present on the host and must actually start. When one is absent, or present but fails to activate, the run is refused before it starts with run error kind `RequirementsUnmet`. The requirements notice names each missing capability by its capability id on its own line, `- missing required capability: {id}`:

````text
the environment cannot satisfy this prompt:
- missing required capability: promptforge/web
````

When a required capability fails to start, the host also logs a line naming it. That line is the host's own log, not a checkpoint from the prompt.

An optional capability is one the prompt can run without. When the host lacks it, the host logs a line naming it, skips it, and the run continues. When it is present but fails to start, it contributes no tools, the run continues, and the failure shows up only in the host's log line.

Declare only capabilities that can activate together. The capabilities themselves declare which others they conflict with, so which pairs conflict depends on the capabilities your host provides; the `promptforge/web` capability conflicts with none. When two declared capabilities conflict, the run is refused with run error kind `RequirementsUnmet` and the notice line `- conflicting capabilities: {first} and {second} cannot be activated together; declare one or the other`, where `{first}` is the one declared earlier. For a host whose `acme/bashkit` and `acme/terminal` capabilities conflict, a prompt that declares `acme/bashkit` first gets this notice:

````text
the environment cannot satisfy this prompt:
- conflicting capabilities: acme/bashkit and acme/terminal cannot be activated together; declare one or the other
````

Conflict detection works both ways and applies to the whole pair. The conflict is found whichever of the two capabilities declares it, it is reported once, naming both in declaration order, and neither capability of the pair activates or contributes tools.

## Filling tool slots and model roles

After activation, prepare fills the prompt's tool slots and model roles with what the host has, and checks each one. A `tools:` entry is a tool slot: an alias the prompt uses, mapped to a tool path such as `promptforge/web/fetch`, whose first two segments name the capability that contributes it ([Tool slots and Tool objects](12-tools.md#tool-slots-and-tool-objects)). A `models:` entry is a model role, and its `keywords` and `min_context` state what the role needs from its model ([Keywords and the thinking switch](10-models.md#keywords-and-the-thinking-switch)). Once filled, a slot or role is bound.

The bindings come only from the frontmatter, never from Lua. They are made once, before the run starts, and stay fixed for the whole run. Every section, the H1 pass included, sees the same bound tools and models, and Lua only chooses among them.

### Tool slots

A tool slot whose tool path names a capability that contributed no tools refuses the run before it starts, with run error kind `RequirementsUnmet`. The notice line `- missing required capability: {capability}` names the capability part of the tool path, so the slot `fetch: promptforge/web/fetch` names `promptforge/web`. Here the slot's capability is not declared at all:

````markdown
---
name: orphan-slot
description: Binds a tool whose capability is not declared
promptforge: 0
tools:
  fetch: promptforge/web/fetch
---

# Orphan Slot

## Only

```lua
return 'done'
```
````

Its outcome is failed, with run error kind `RequirementsUnmet` and exactly this message:

````text
the environment cannot satisfy this prompt:
- missing required capability: promptforge/web
````

When several slots share one missing capability, the notice names it only once. Slots are checked in sorted alias order, so each missing capability appears where its first slot falls in that order.

A slot whose capability is active but contributed no tool at that path is a different case. The capability is not missing, so the run starts: the slot stays unbound, and the host logs a warning. The slot fails only when a section tries to offer that alias to the model, with the message `tools.add alias "{alias}" is not a bound tool slot`, for example `tools.add alias "search" is not a bound tool slot`.

### Model roles

Before the run starts, prepare checks each model role's `min_context` and its hard keywords, `thinking` and `no-thinking`, against the model bound to that role. Prepare checks and never repairs: it reports every failure as a line in the requirements notice, naming the role label and giving what was required against what the model has, and it never looks for a different model.

`min_context: {tokens}` requires a minimum context window. When the bound model's context is smaller, the run is refused with run error kind `RequirementsUnmet` and the line `- role '{label}': requires a context of at least {min} tokens; the current model provides {actual}`. The comparison is strict, so a model whose window equals the minimum passes.

The thinking keywords are checked against the bound model's thinking capability, which the messages name `Never`, `Always`, or `Switchable`. The line for either keyword reads `- role '{label}': requires '{keyword}'; the current model's thinking capability is {capability}`.

- `thinking` requires a model that can think. A model whose thinking capability is `Never` refuses the run; `Always` and `Switchable` models satisfy it.
- `no-thinking` requires a model that can answer without thinking. A model whose thinking capability is `Always` refuses the run; a `Switchable` model passes and is asked to turn thinking off.

Only `min_context` and these two keywords can produce a role line. The other keywords, `frontier`, `fast`, `small`, `creative`, and `chat`, state intent and are never checked.

Prepare does not stop at the first failure. Every unmet requirement across all roles appears in one refusal, each failed check on its own line, so a role that fails both checks gets two lines. Roles are listed in sorted label order, and within a role the context line comes first, then the keyword lines in the order the keywords are declared:

````markdown
---
name: deep-analysis
description: Needs a large context and a thinking model
promptforge: 0
models:
  analyst:
    keywords: [frontier, thinking]
    min_context: 200000
    description: Deep analysis
---

# Deep Analysis

## Only

```lua
return 'done'
```
````

Against a model with a 32000-token context whose thinking capability is `Never`, the run fails with run error kind `RequirementsUnmet` and this message:

````text
the environment cannot satisfy this prompt:
- role 'analyst': requires a context of at least 200000 tokens; the current model provides 32000
- role 'analyst': requires 'thinking'; the current model's thinking capability is Never
````

Against a model with a 200000-token context whose thinking capability is `Always`, the same prompt's run result is:

````text
done
````

### A prompt with no requirements

A prompt that declares no capabilities, tool slots, or model roles, and never calls a model, runs as is, with nothing bound:

````markdown
---
name: no-requirements
description: Declares nothing and calls no model
promptforge: 0
---

# Test prompt

## Only

```lua
return 'no capabilities'
```
````

Its run result:

````text
no capabilities
````

## When a run cannot start

When prepare finds any gap, the run is refused before any section runs, and its message is the requirements notice. The notice has a fixed layout: the header line `the environment cannot satisfy this prompt:`, then one line starting with `- ` for each gap. Missing capabilities come first, then capability conflicts, then unmet model role requirements:

````text
the environment cannot satisfy this prompt:
- missing required capability: {id}
- conflicting capabilities: {first} and {second} cannot be activated together; declare one or the other
- role '{label}': requires a context of at least {min} tokens; the current model provides {actual}
- role '{label}': requires '{keyword}'; the current model's thinking capability is {capability}
````

One refusal names every gap at once. The gaps found during capability activation and the gaps found while filling tool slots and model roles merge into one notice, and a capability that both steps report missing is listed only once. With no capabilities on the host, the `fetch: promptforge/web/fetch` slot from [Filling tool slots and model roles](#filling-tool-slots-and-model-roles) gives a one-line notice, even though both steps find `promptforge/web` missing.

A prompt runs normally when every required capability is present, no two declared capabilities conflict, and every model role's requirements are met. The refusal happens only when at least one of those fails. The `analyst` prompt above runs to `done` on a 200000-token model whose thinking capability is `Always`.

The run error kind `RequirementsUnmet` covers both ways a prompt's preconditions can fail: a requirement the host cannot satisfy, found at prepare, and an uncaught Lua error in the H1 pass, the [hard gate](#the-h1-pass). The message tells them apart: a refusal at prepare is the requirements notice, and a failed gate is the Lua error text. [How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified) lists the other run error kinds.

A prompt can also fail to start for reasons outside the notice: its `promptforge` version is missing or unsupported ([The promptforge version](02-file-structure.md#the-promptforge-version)), or its store cannot be set up ([Store errors](09-the-store.md#store-errors)). Such a prompt runs none of its sections and reports nothing; that failure is the run's whole outcome. A file without the `promptforge:` key parses but never runs.

## Waiting and reproducibility

Host work, the work the host does for a run ([The prompt and its host](01-what-a-prompt-is.md#the-prompt-and-its-host)), is done by the host while only the calling chain waits. Model requests, tool calls, `user_input()` requests, and store operations are all host work. While one chain waits for its answer, other chains keep running. For example, `store.write('notes.md', 'kept')` issues one piece of host work, and its chain waits for the answer before its next line runs. `user_input()` asks the operator for text ([Asking the operator with user_input](05-lua-environment.md#asking-the-operator-with-user_input)), `store.write` saves a file in the run's store ([What the store is](09-the-store.md#what-the-store-is)), and a script calls tools by alias ([Calling tools from Lua](12-tools.md#calling-tools-from-lua)). Tasks, sections started to run beside the section that started them, are chains that keep running this way ([Starting a task](15-tasks.md#starting-a-task)).

Runs are reproducible. Each run has a seed and a start instant, both supplied by the host; a prompt cannot set either one. Lua in a prompt has no clock or random source of its own, so the same prompt with the same seed, the same start instant, and the same model answers, as a replay has, produces byte-identical result text. It also produces the same `sys.when`, the run's start time as Lua sees it, and the same host work in the same order. The same holds for the run's events ([Reading a task's history](16-task-events.md#reading-a-tasks-history)) and for the random nonce inside each untrusted envelope ([Wrapping untrusted text](09-the-store.md#wrapping-untrusted-text)); a different seed gives a different nonce.

A task's record does not depend on how fast, or in what order, model answers arrive. Answering the model requests one at a time, all at once in the order they were issued, or all at once in reverse gives the same result text, the same events for each task, and the same host work for each task. Only the interleaving across tasks differs.
