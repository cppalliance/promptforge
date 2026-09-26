# Models

A prompt never names a concrete model. It declares the roles it needs, such as a writer or an analyst, and the host fills each role with its current model before the run starts, so the same prompt runs on whatever model the host provides. This chapter shows you how to declare roles and their requirements, choose the role each section uses, inspect a role through its model handle, set sampling options, and send text to the model with `models.infer`.

## Model roles at a glance

The smallest prompt that talks to a model declares one role, makes it the default, and sends a section's prose to the model:

````markdown
---
name: haiku
description: Writes a haiku about autumn leaves
promptforge: 0
models:
  writer: {}
---

# Haiku

```lua
models.default('writer')
```

## Write

Write a haiku about autumn leaves.

```lua
return models.infer(prose)
```
````

The `models:` frontmatter key declares the model roles the prompt needs. Its value is a map from a role label, a name local to the prompt, to that role's declaration, and `writer: {}` declares a role labeled `writer` with no settings. Lua refers to a role by its label.

At [prepare](04-how-a-prompt-runs.md#filling-tool-slots-and-model-roles), the step before the run, the host fills every declared role with its one current model, the model the host chose from its model catalog. A prompt names roles, never a concrete model, and it can declare as many roles as it needs. The host has one current model, so every role is bound to that same model, and what sets roles apart is their settings, which the rest of this chapter covers.

The block in the H1 body runs in the [H1 pass](04-how-a-prompt-runs.md#the-h1-pass), before any section, and `models.default('writer')` there makes `writer` the prompt-wide default role. Every section that makes no selection of its own uses the default, and so does every model call there that names no model. By convention you call `models.default` from the H1 body, but it works from any section, because the whole run shares one set of roles and one default.

In `## Write`, `models.infer(prose)` sends the section's prose, which the block reads through [the `prose` global](03-blocks-and-prose.md#the-prose-global), to the model. That is one round: one request to the model and the model's reply, which comes back as a plain Lua string. `models.infer` is a [suspending call](05-lua-environment.md#calls-that-wait-and-errors-that-raise), so the block pauses while the round runs and resumes with the reply. The block returns the reply, and [the scalar return rule](04-how-a-prompt-runs.md#block-and-section-returns) makes it the run result.

You can reach the `models` table from the Lua of every section, from the blocks of the H1 body, and from the [shared library](03-blocks-and-prose.md#the-shared-library). Its functions are `models.use`, `models.default`, `models.get`, and `models.infer`. It also holds `models.loop`, which runs several rounds as a conversation and is taught in [Conversations](11-conversations.md#a-first-conversation).

`models.use(label)` selects a role for the rounds that follow in the current section. That choice is the section's selection, and it overrides the prompt-wide default for that section only:

````markdown
---
name: tagline
description: Drafts a bakery tagline and critiques it
promptforge: 0
models:
  writer: {}
  critic: {}
---

# Tagline

```lua
models.default('writer')
```

## Draft

Write a one-line tagline for a neighborhood bakery.

```lua
var.draft = models.infer(prose)
```

## Critique

```lua
models.use('critic')
return models.infer('Critique this tagline in two sentences: ' .. var.draft)
```
````

`## Draft` makes no selection, so its round runs on the default role `writer`. Its block keeps the reply in [`var`](05-lua-environment.md#keeping-values-in-var), which holds values from one block to the next along the walk, and returns nothing, so the [walk](04-how-a-prompt-runs.md#the-section-walk) falls through to `## Critique`. There `models.use('critic')` selects `critic` for that section's round, and the section returns the critique as the run result.

## Declaring roles

A role that sets nothing is an empty map, as `writer: {}` shows. It has no keywords, no minimum context, and no description, and it still counts as a role. To set something, give the role a map with any of three optional keys:

````yaml
name: market-report
description: Writes a short market report
promptforge: 0
models:
  analyst:
    keywords: [frontier, thinking]
    min_context: 200000
    description: Deep analysis of the quarterly figures
  scout:
    keywords: [fast]
````

- `keywords` is a YAML list of words from a closed vocabulary of seven, kept in the order you write them. The next section covers each word. Leaving it out means no keywords.
- `min_context` is the smallest context window the role accepts, a whole number of tokens from 1 to 4294967295. Leaving it out means no minimum.
- `description` is a string that says what the role is for. It is separate from the prompt's own top-level `description:`, and a role without one takes the bound model's own catalog description instead.

Flow style works too, so a role fits on one line, as in `analyst: { keywords: [no-thinking, creative, chat], min_context: 32000, description: deep reasoning }`. Leave `models:` out of the frontmatter to declare no roles at all.

Give every role under `models:` a distinct label, written in the [name grammar](02-file-structure.md#names-for-aliases-roles-and-args) that every prompt-local name follows: an ASCII letter followed by up to 63 ASCII letters, digits, `_`, or `-`.

### Declaration errors

A mistake inside `models:` fails the parse with a [`Frontmatter`](17-limits-and-errors.md#parse-error-kinds) parse error that reports the line and column of the mistake:

- A label used twice fails with ``duplicate model role label `{key}`: contract map keys must be unique``, which names the label.
- A label outside the name grammar fails with ``invalid model role label `{key}`: expected [A-Za-z][A-Za-z0-9_-]{0,63}``, which names the label and the grammar.
- Any other key inside a role, a keyword outside the seven, or a `min_context` of zero also fails the parse, with a message from the YAML reader.

## Keywords and the thinking switch

The seven keywords come in two kinds. The hard keywords are `thinking` and `no-thinking`, and the soft keywords are `frontier`, `fast`, `small`, `creative`, and `chat`. Every keyword is written in kebab-case, and any other word in `keywords:` fails the parse with a `Frontmatter` parse error.

Soft keywords record what you intend the role for. They are accepted, but prepare never checks them against the model and never refuses a run because of them. A hard keyword sets the role's thinking switch for every round under the role:

````markdown
---
name: boiling-point
description: States a fact without extended thinking
promptforge: 0
models:
  responder:
    keywords: [no-thinking, fast]
    description: Short direct replies
---

# Boiling point

```lua
models.default('responder')
```

## Ask

In one sentence, what is the boiling point of water at sea level?

```lua
return models.infer(prose)
```
````

`thinking` asks for thinking on, `no-thinking` asks for thinking off, and a role with neither leaves the model's own default. Every model in the host's catalog has one of three thinking modes: it never thinks, it always thinks, or it can switch thinking on and off per request. The switch takes effect on a model that can switch, and there every round under `responder` asks for thinking off, including rounds in sections that reach the role only through the prompt-wide default. If a role lists both hard keywords, the later one in the list sets the switch. Here `fast` records intent and changes nothing.

## Requirements at prepare

Hard keywords and `min_context` are requirements. At prepare, the host checks each role against the model it is bound to, and each unmet requirement becomes one line of the [requirements notice](04-how-a-prompt-runs.md#when-a-run-cannot-start), the text prepare writes when it refuses to start the run. A refused run fails before any of the prompt's blocks run, with run error kind [`RequirementsUnmet`](17-limits-and-errors.md#how-a-failed-run-is-classified). Each line names the role by its label:

- `thinking` needs a model that can think. It is unmet only when the bound model never thinks, and its line is `role '{role}': requires 'thinking'; the current model's thinking capability is Never`.
- `no-thinking` needs a model that can reply without thinking. It is unmet only when the bound model always thinks, and its line is `role '{role}': requires 'no-thinking'; the current model's thinking capability is Always`.
- `min_context` is unmet when the bound model's context window is smaller than the minimum, and its line is `role '{role}': requires a context of at least {required} tokens; the current model provides {actual}`. A window equal to the minimum passes.

For example, a role labeled `analyst` with `min_context: 200000`, on a host whose current model has a 32000-token context window, stops the run with this notice:

````text
the environment cannot satisfy this prompt:
- role 'analyst': requires a context of at least 200000 tokens; the current model provides 32000
````

Both thinking lines have the form `role '{role}': requires '{required}'; the current model's thinking capability is {actual}`, where `{required}` is the keyword and `{actual}` is `Never` or `Always`. A model that can switch thinking satisfies both hard keywords, so a role that lists both still passes on it, and its rounds follow the later keyword. Soft keywords are never checked. Prepare checks only hard keywords and `min_context`, and only when the host provides a current model.

## Choosing a section's model

A round that names no model, such as `models.infer(prose)`, runs on the section's model, which the section picks in a fixed order:

1. The section's own selection, from its latest `models.use` call.
2. Otherwise the prompt-wide default, from `models.default`.
3. Otherwise no model at all.

Every section starts with no selection, so a selection never reaches past its own section. Call `models.use` again to switch roles partway through a section:

````lua
models.use('writer')
local draft = models.infer('Write a tagline for a neighborhood bakery.')
models.use('critic')
return models.infer('Critique this tagline in two sentences: ' .. draft)
````

The selection is read when each round starts, so the latest `models.use` call steers the next round. The first round runs on `writer`, the second on `critic`, and the section's result is the second reply. Selecting the same label again is allowed.

### The missing-model error

Give a section a selection or a prompt-wide default before it runs a round. A round with neither fails with the missing-model error, `model binding required for section {section}`, where `{section}` is the section's heading text, or the H1 title for a round in the H1 pass. For example, a section `## Only` whose block runs `return models.infer(prose)`, in a prompt that declares `writer` but never selects it or makes it the default, fails with:

````text
model binding required for section Only
````

Left uncaught, the missing-model error fails the run with run error kind [`Binding`](17-limits-and-errors.md#how-a-failed-run-is-classified), in the H1 pass as everywhere else. [`pcall`](05-lua-environment.md#catching-and-inspecting-errors) catches it as an error value of kind `internal`:

````lua
local ok, result = pcall(models.infer, prose)
if ok then
  return result
end
return 'skipped (' .. result.kind .. ')'
````

In a section with no selection and no default, this block returns `skipped (internal)`. A conversation run with [`models.loop`](11-conversations.md#a-first-conversation) picks its model in the same order and raises the same error. Prose that is never sent to a model needs no model, and a prompt that makes no round runs even when the host provides no model at all.

### One default for the whole run

Every section of the run, and the H1 body too, sees the same roles and the same prompt-wide default, because the run shares them instead of copying them for each section. The default is set once per run, and calling `models.default` again with the same label does nothing. That makes it safe to set the default from the shared library, the `lua shared` fence in the H1 body, whose code [replays](03-blocks-and-prose.md#how-the-shared-library-loads) at the start of every section:

````markdown
```lua shared
models.default('writer')
```
````

Naming a different label once the default is set fails with `models.default is already "{existing}": the prompt-wide default cannot change mid-run`, which names the current default.

### Labels that name no bound role

Roles come only from the frontmatter, and Lua never creates one. `models.use` and `models.default` each take a label that names a bound role, and otherwise fail:

- `models.use label "{label}" is not a bound model role` from `models.use`, naming the label.
- `models.default label "{label}" is not a bound model role` from `models.default`, naming the label. The default stays unchanged.

When the host runs a prompt with no current model, prepare has nothing to fill or check and refuses nothing, so the declared roles stay unbound. Selecting any of them at run time, with `models.use` or `models.default`, then fails with the not-a-bound-role error that names the label.

These errors, like the `models.default is already` error, are error values of kind `lua`. Left uncaught, they end the run like any other Lua failure, with run error kind [`Lua`](17-limits-and-errors.md#how-a-failed-run-is-classified), except in a `lua` fence of the H1 body, where [the H1 pass](04-how-a-prompt-runs.md#the-h1-pass) turns an uncaught Lua failure into a `RequirementsUnmet` refusal whose notice is the error text.

## Model handles

A model handle is a Lua value that stands for one bound role. `models.use`, `models.default`, and `models.get` each return one, and all three return the same kind of value, whose fields describe the role. `models.get(label)` returns a role's handle without changing the section's selection, and the handle describes the role's own settings whatever the section has selected.

A handle is userdata with readable fields and no methods, so `type(h)` is `'userdata'`. To run a model with a handle, pass it as the first argument to a `models` function:

````lua
local analyst = models.get('analyst')
return models.infer(analyst, prose)
````

This round runs on the `analyst` role even when the section has selected another role or has no model of its own.

### Role globals

Each bound role is also a role global: a bare Lua global named after the role's label that holds the role's handle. The block above works without `models.get`:

````lua
return models.infer(analyst, prose)
````

Role globals are set after the shared library loads, so a declared label wins over a same-named global that shared code defines. For the same reason, top-level code in the shared library cannot read role globals yet, while a function it defines can read them when a section calls it. When a role label is also a key under `tools:`, the bare global with that name holds the model handle.

### Keeping the default's handle

`models.default` returns the default role's handle, so you can keep it to inspect the role or to pass it to `models.infer`:

````lua
local writer = models.default('writer')
var.capital = models.infer(writer, 'Reply with one word: the capital of Peru.')
````

The shared library can keep the handle in a global the same way. Called as a statement, `models.default('writer')` only sets the default.

### Frozen and read-only

A handle is a frozen snapshot. Its field values are fixed when the handle is made and stay the same for as long as you hold it, whatever the section selects afterwards. Every field is read-only, and assigning to one raises a Lua runtime error.

### Model ids from the host

When the host runs the prompt with a host-state snapshot, the one [`ui()`](05-lua-environment.md#host-state-with-ui) reads, `models.get` also accepts a model id from the host's catalog, as in `models.get(ui().selected_model)`, and returns a handle for a model the prompt never declared. The `ui` global exists only when the host gives a snapshot, so this block tests for it first:

````lua
local id = ui and ui().selected_model
if id then
  return models.infer(models.get(id), prose)
end
return models.infer(prose)
````

This is the only way a prompt reaches a model other than the host's current model. A declared role whose label matches the id still wins. The handle's `name`, `label`, `model_id`, and `description` are all the id, its `capabilities` is empty, its `context` is 8192, and it has no sampling or thinking settings. It passes to `models.infer` like any other handle. Only `models.get` has this fallback, and `models.use` and `models.default` always need a bound role.

A model id is any non-empty text with no control characters, and it may contain `/`, `.`, `:`, and non-ASCII letters, as in `qwen/qwen3-8b`. An empty id, or one with a control character, fails with `models.get model id "{id}" is invalid: {reason}`, which names the id and the reason.

### Label errors

A label passed to `models.use` or `models.default`, or to `models.get` outside the model id fallback, follows the same grammar as a role label, 1 to 64 bytes in all. `models.use` and `models.default` check the grammar and then look for the bound role. `models.get` looks for a declared role first, and when none matches it treats the name as a model id if the host gave a snapshot, or otherwise checks the grammar and then reports the missing role. These calls fail with:

- `invalid alias "{name}": expected [A-Za-z][A-Za-z0-9_-]{0,63}` for a name outside the grammar, which names the rejected name and the grammar.
- `models.get alias "{alias}" is not a bound model role` for a `models.get` label that names no bound role, naming the label. The error is the same wherever the call runs, including a run with no current model, where no role is bound.
- The not-a-bound-role errors for `models.use` and `models.default`, listed under [Choosing a section's model](#choosing-a-sections-model).

Every rejected `models.use`, `models.default`, or `models.get` call raises an error value of kind `lua` whose message is the rejection text, and `tostring(err)` gives that text. Catch it with `pcall` to fall back instead of failing, for example when the role's label arrives as the run's [argument string](06-arguments.md#input-basics):

````lua
local ok, h = pcall(models.get, args)
if not ok then
  return 'unknown role: ' .. tostring(h)
end
return models.infer(h, prose)
````

## Sampling options

Pass an options table as the second argument of `models.use` to set sampling for that selection:

````markdown
---
name: slogan
description: Writes a slogan at a low temperature
promptforge: 0
models:
  writer: {}
---

# Slogan

## Write

Write a slogan for a bicycle repair shop.

```lua
models.use('writer', { temperature = 0.3, max_tokens = 256 })
return models.infer(prose)
```
````

The round asks the model for a temperature of 0.3 and caps its reply at 256 generated tokens. The handle that `models.use` returns reads the values back, so its `temperature` is `0.3` and its `max_tokens` is `256`. An option you leave out stays unset and reads `nil`: after `local h = models.use('writer', { temperature = 0 })`, `h.max_tokens` is `nil`.

The options hold for every following round in the section that runs on that selection: a round that names no model, such as `models.infer(prose)`, and a round on the handle that `models.use` returned. They also reach the rounds of a conversation that [`models.loop`](11-conversations.md#a-first-conversation) runs on the selection. They never apply to rounds on the prompt-wide default or on a `models.get` handle, and a later plain `models.use(label)` clears them.

A round asks the model only for what its role and selection set: the role's thinking switch, plus `temperature` and `max_tokens` when `models.use` options set them. A round on the prompt-wide default sends no temperature and no token cap.

### Option values

- `temperature` is a finite Lua number or integer from 0.0 to 2.0 inclusive, so `0`, `0.7`, and `2.0` all work.
- `max_tokens` is a whole number from 1 to 4294967295, written as a Lua integer or as a float with no fractional part.
- The table names only `temperature` and `max_tokens`, as string keys.

`models.use` takes at most two arguments, a label and an optional options table. Leaving the table out, or passing `nil`, means no options.

### Option errors

A bad options argument fails the call with an error value of kind `lua`, and each message names the option and what was required versus what was given:

- `models.use option temperature must be a number, got {type}` for a `temperature` that is not a number.
- `models.use option temperature must be finite, got {value}` for NaN or infinity.
- `models.use option temperature {value} is outside the supported range [0.0, 2.0]` for a number outside the range.
- `models.use option max_tokens must be an integer in [1, 4294967295], got {actual}` for any other `max_tokens` value, where `{actual}` is the number, or the type of a non-number.
- `models.use option "{name}" is unknown: expected temperature or max_tokens` for any other option name.
- `models.use option names must be strings, got {type}` for a key that is not a string, including a positional entry.
- `models.use options must be a table, got {type}` for a second argument that is not a table.
- `models.use takes at most 2 arguments, got {count}` for a call with a third argument.

A rejected call leaves the section's selection unchanged, because the label and option checks all run before the selection is recorded. When one table has several problems, every run reports the same first error: non-string keys are checked first, grouped by type name, then string names in bytewise order, whatever Lua's table iteration order. So `{ temperature = 3, max_tokens = 0 }` always reports the `max_tokens` error, because `max_tokens` sorts before `temperature`.

## Handle fields

A handle has exactly nine fields, all read-only:

| Field | Value |
|---|---|
| `name` | The role label |
| `label` | The role label, always the same string as `name` |
| `description` | The role's `description:`, or else the bound model's catalog description, which can be empty |
| `capabilities` | The role's keywords, as a Lua sequence of kebab-case strings in declaration order |
| `model_id` | The bound model's catalog id alone, such as `claude-sonnet-4-6` |
| `context` | The bound model's context window in tokens, an integer of at least 1 |
| `thinking` | The role's thinking switch: `true` for `thinking`, `false` for `no-thinking`, `nil` for neither |
| `temperature` | The `temperature` option on a handle from `models.use` with options, `nil` otherwise |
| `max_tokens` | The `max_tokens` option, a positive integer, on a handle from `models.use` with options, `nil` otherwise |

Read the fields to inspect a role without running a round:

````markdown
---
name: inspect
description: Reports the settings of the analyst role
promptforge: 0
models:
  analyst:
    keywords: [frontier, thinking]
    description: Deep analysis
---

# Inspect

## Show

```lua
local h = models.get('analyst')
return h.label .. '|' .. h.model_id .. '|' .. table.concat(h.capabilities, ',')
```
````

On a host whose current model is `claude-sonnet-4-6`, the run result is:

````text
analyst|claude-sonnet-4-6|frontier,thinking
````

`model_id` is the model's id, distinct from the role label. Every declared role is bound to the host's one current model, so `model_id` reads the same on every role's handle, while fields such as `description`, `capabilities`, and `thinking` show how the roles differ.

Each read of `capabilities` builds a fresh table, so changing the table you get back leaves the handle unchanged. `temperature` and `max_tokens` read `nil` on every handle for a declared role, except a handle that `models.use` returned with options, because a role declares no sampling settings of its own. Reading any key other than the nine raises a Lua runtime error that names the key: `attempt to get an unknown field '{key}'`.

## Running a round with models.infer

`models.infer` takes the prompt text, with an optional model handle in front:

````lua
models.infer(prompt)
models.infer(handle, prompt)
````

Without a handle, the round runs on the section's model: the role `models.use` selected, or else the prompt-wide default. With a handle first, the round is pinned to that role and runs on the handle's frozen settings instead of the section's model, which works even when the section has no selection and there is no prompt-wide default:

````lua
local summary = models.infer(prose)
return models.infer(models.get('analyst'), 'Name the weakest claim in this summary: ' .. summary)
````

The first round runs on the section's model and the second on `analyst`.

Each call is one round that starts fresh. The model receives the prompt text alone, with nothing from earlier rounds or from the rest of the section, and the round offers the model no tools, even when the prompt declares tools. When a Lua block needs a round that can use tools, it runs a section with [`call`](08-jump-and-call.md#called-chains) instead.

The reply comes back whole as the call's return value, and none of it appears in the run's live output while the model is still writing. It holds only the reply text, and a model's reasoning text never becomes part of it. `models.infer` changes nothing else in the section, so [`sys`](05-lua-environment.md#run-metadata-in-sys) reads the same before and after the round. Inside a [task](15-tasks.md#starting-a-task), `models.infer` keeps the task waiting until the reply arrives, and while it waits the task shows `blocked=chat`.

### Bad arguments

Bad arguments fail the call with an error value of kind `lua`:

- `models.infer takes (handle?, prompt)` for a call with more than two arguments.
- `prompt must be a string, got {type}` for a prompt that is not a string, with `got nil` when the prompt is missing.
- `prompt must be a valid UTF-8 string` for a prompt string with invalid bytes.
- `models.infer handle must be a model handle, got {type}` when two arguments are given and the first is not userdata.
- `models.infer handle must be a model handle`, with no type named, when two arguments are given and the first is some other kind of userdata.

### When a round goes wrong

If the model replies with tool calls anyway, the call fails with an error value of kind `lua` whose message is `model inference received tool calls but no tools were advertised`. The text is the same for both forms and does not name `models.infer`.

A round that fails raises the round's own error. `pcall` sees it with kind `internal`, and left uncaught it fails the run with run error kind [`Completion`](17-limits-and-errors.md#how-a-failed-run-is-classified).

A round cut off at the generation cap, such as a `max_tokens` option, still returns the text the model produced up to that point. The run records every round as round events under the current section, marking a failed round as failed and a cut-off round as truncated, and [Task Events](16-task-events.md#model-round-events) describes those events.

## The bound model in sys.model

`sys.model` holds the catalog model id of the section's model, such as `claude-sonnet-4-6`, never the role label. Read it in Lua, or in prose with the [placeholder](07-substitution.md#values-from-var-sys-and-lua-globals) `{{ sys.model }}`:

````markdown
---
name: model-report
description: Reports the model a section runs on
promptforge: 0
models:
  writer: {}
---

# Model report

```lua
models.default('writer')
```

## Report

```lua
tools.add_local('stamp', 'Returns a fixed stamp', {}, function() return 'ok' end)
tools.call('stamp')
```

Model in use: {{ sys.model }}.

```lua
return prose
```
````

The first block makes the section's first tool call, here to a small tool written in Lua, and [Tools](12-tools.md#calling-tools-from-lua) teaches both parts. On a host whose current model is `claude-sonnet-4-6`, the run result is `Model in use: claude-sonnet-4-6.`

The field appears at the section's first tool call, made either from a Lua block or at the model's request during a conversation; whichever tool the first call runs, it counts. At that moment `sys.model` takes the section's model, its selection or else the prompt-wide default, and it stays fixed for the rest of the section. A round by itself never sets it, and a section that makes no tool call, or has no model when its first tool call runs, never gets the field.

Reading `sys.model` before the section's first tool call fails with `unknown sys field 'model'`, even when the H1 pass set a prompt-wide default. That covers a read in the section's first Lua block ahead of any tool call and a read inside a shared-library function called from there, and the read fails the same way anywhere in a section that makes no tool call or had no model at its first tool call. Left uncaught in a section, the error fails the run with run error kind [`Lua`](17-limits-and-errors.md#how-a-failed-run-is-classified). To read the field only when it is there, wrap the read in `pcall`:

````lua
local ok, id = pcall(function() return sys.model end)
return ok and id or 'no tool call yet'
````
