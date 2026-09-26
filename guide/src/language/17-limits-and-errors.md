# Limits and Errors

This chapter is the map for when a prompt runs long or goes wrong. It gathers every limit a run observes in one table, shows what happens when Lua code runs into each one, and gives the full classification of failures: the parse error kinds a file can fail with before it runs, the error kinds `pcall` sees inside Lua, and the run error kind a failed run reports. It ends with the host cancel, the one way a run stops without failing. With it, any failure message from any chapter tells you what broke and where to look.

## Limits at a glance

Every run observes six limits. These are their defaults:

| Limit | Default | Applies to | Set by |
|---|---|---|---|
| Round cap | 24 tool rounds | each `models.loop` call | `max_tool_iterations:` in the frontmatter, or the host |
| Concurrency cap | 8 arms at once | fanout arms running together | the host |
| Response cap | 16 MiB (16,777,216 bytes) | each model reply | the host |
| Memory ceiling | 64 MiB (67,108,864 bytes) of Lua heap | each section VM | the host |
| Log event quota | 1024 `log` calls | each section VM | the host |
| Receive timeout | 120 seconds | each wait for the next piece of a model reply | the host |

The log byte quota follows from the log event quota: 256 bytes for each call the log event quota allows, so 262,144 bytes by default. No limit counts Lua instructions.

One set of limits applies to the whole run, the H1 pass and every section included. A run uses the defaults unless the host that runs it sets its own values, so the limits a given run observes can differ from this table.

From inside a prompt, only the round cap can change. The frontmatter key `max_tool_iterations` takes a whole number from 1 to 1000 and caps each `models.loop` call separately ([The round cap](11-conversations.md#the-round-cap)):

````yaml
---
name: researcher
description: Researches a topic with tools
promptforge: 0
max_tool_iterations: 50
---
````

A value outside that range fails the parse with parse error kind `Frontmatter` and one of these messages, where `{raw}` is the value as written:

````text
max_tool_iterations must be a positive integer (>= 1), got {raw}
max_tool_iterations must be <= 1000, got {raw}
````

No frontmatter key sets the other five limits. The concurrency cap is taught with [fanout](14-fanout.md#concurrency), and the rest of this chapter covers the memory ceiling, the log quotas, the response cap, and the receive timeout. The web fetch tool has a policy of its own, whose values are also defaults set by the host and that a prompt cannot change ([The fetch policy](13-web-fetch-and-search.md#the-fetch-policy)).

## How failures are reported

The whole prompt file is checked before anything runs. The parser validates the frontmatter, the headings, the fences, and the sections, and compiles every Lua block: the shared library, each block in the H1 body, and each section block. A structural error or a Lua syntax error anywhere in the file, even in the last section, stops the prompt before any section runs.

Failures fall into four families, each with its own vocabulary:

| Family | When it happens | What you see |
|---|---|---|
| Parse failure | before anything runs | one of five parse error kinds |
| Error value | inside Lua, at the call that failed | an error value whose `kind` is one of twelve lowercase tags |
| Failed run | when the file fails to parse, prepare refuses the run, or a failure goes uncaught | one of thirteen run error kinds |
| Cancelled outcome | when the host cancels | no error kind at all |

A parse failure has exactly one of five parse error kinds: `Frontmatter`, `Structure`, `Fence`, `List`, or `Lua` ([Parse error kinds](#parse-error-kinds)).

Inside Lua, a failure is an error value ([Catching and inspecting errors](05-lua-environment.md#catching-and-inspecting-errors)), and its `kind` is always one of `lua`, `internal`, `cancelled`, `context_exhausted`, `empty_model_reply`, `tool_loop_exhausted`, `tasks_live`, `task_not_owned`, `task_consumed`, `out_of_scope_tool`, `unbound_tool`, or `tool`. Catch it with `pcall` and branch on `err.kind`:

````lua
local ok, reply = pcall(models.infer, prose)
if not ok then
  local err = reply
  if err.kind == 'internal' then
    return 'model unavailable: ' .. err.message
  end
  error(err)
end
return reply
````

Here an `internal` failure, such as a model call the host could not complete, becomes the section's result, and any other kind is raised again unchanged before any other suspending call, so the run ends exactly as it would have without the `pcall`.

A failed run reports one run error kind naming what failed, from a fixed set of thirteen: `Parse`, `Version`, `Binding`, `Completion`, `Tool`, `Store`, `Determinism`, `Lua`, `Quota`, `ContextExhausted`, `Input`, `Internal`, and `RequirementsUnmet` ([How a failed run is classified](#how-a-failed-run-is-classified)). The host decides how it shows the run error kind, the message, and, for a parse failure, the location to the person running the prompt.

A run the host cancels is not a failed run. A running prompt can be stopped at any point, even inside a Lua loop that never waits on the host, and the run then ends with the cancelled outcome ([Failure and cancellation](04-how-a-prompt-runs.md#failure-and-cancellation)), a clean stop rather than a failure. The host triggers the cancel, for example on Ctrl-C. A prompt cannot cancel its own run; it only observes the outcome ([Cancelling a run](#cancelling-a-run)).

## Lua block budgets

Each section VM runs its Lua under the memory ceiling and two log quotas. They are applied right after the section VM is built, before any setup step, so they cover every block from its first instruction, the [shared library replay](03-blocks-and-prose.md#how-the-shared-library-loads) and the H1 pass included. Each section VM starts with its own full allowance.

### No instruction limit

A Lua block has no instruction-count limit, so long loops are legal. A block runs until it finishes, runs past the memory ceiling, runs out a log quota, or the run is cancelled. This bounded loop simply runs to the end:

````markdown
---
name: counter
description: Counts to eight million
promptforge: 0
---

# Counter

## Count

```lua
local n = 0
for i = 1, 8000000 do n = n + 1 end
return n
```
````

Its run result:

````text
8000000
````

Even an endless loop such as `while true do end` is legal. Only a host cancel stops it, within about 10,000 instructions ([Cancelling a run](#cancelling-a-run)).

### The memory ceiling

Each section VM may use up to 64 MiB of Lua heap by default, or the host's value. An allocation past the memory ceiling is refused as an ordinary Lua error whose message mentions memory. It is not a quota:

- Caught with `pcall`, it is an error value of kind `lua`.
- Left uncaught, it ends the run with run error kind `Lua`, or with `RequirementsUnmet` in the H1 pass, where the Lua error text becomes the requirements notice.
- The section VM is still torn down normally.

A block that keeps adding large strings to a table forever is the usual way to reach it:

````lua
local t, i = {}, 1
while true do
  t[i] = string.rep('x', 16384)
  i = i + 1
end
````

### The log quotas

[`log`](05-lua-environment.md#checkpoints-with-log) records author checkpoints, and each section VM has two quotas for it:

- The log event quota: 1024 `log` calls by default, or the host's value.
- The log byte quota: the log event quota times 256, so 262,144 bytes by default, counted in UTF-8 bytes across all of the section VM's `log` calls. Multibyte text uses it up faster: 256 copies of `é` cost 512 bytes.

Every `log` call counts, calls made while the shared library loads included. Each call is checked in this order:

1. It must have exactly one argument, or it fails with `log expects exactly one argument` and spends nothing.
2. It spends one unit of the log event quota.
3. It is checked against the string type, 256-character, and line-break rules, so a one-argument call that breaks one of them has still spent its unit.
4. It charges the message's UTF-8 byte length to the log byte quota. A message that would overflow the quota is refused whole, and messages logged before it stay recorded.

A call past either quota raises an error instead of recording a checkpoint. What you see depends on whether you catch it:

| Quota | Caught with `pcall`, kind `lua` | Uncaught, run error kind `Quota` |
|---|---|---|
| Log event quota | `lua log event budget exceeded` | `lua log event quota exceeded` |
| Log byte quota | `lua log cumulative byte budget exceeded` | `lua log byte quota exceeded` |

The uncaught message names no prompt line. `Quota` stays `Quota` in the H1 pass too.

This prompt logs more than the default log event quota allows and catches the refusal:

````markdown
---
name: chatty
description: Logs past the log event quota and catches the refusal
promptforge: 0
---

# Chatty

## Steps

```lua
local ok, err = pcall(function()
  for i = 1, 2000 do
    log('step ' .. i)
  end
end)
if not ok then
  return err.kind .. ': ' .. err.message
end
return 'all steps logged'
```
````

With the default quota, the 1025th call is refused, and the run result is:

````text
lua: lua log event budget exceeded
````

Two host-set quotas show the other cases. With a log event quota of 4, the log byte quota is 1024 bytes, so three `log(string.rep('é', 200))` calls of 400 bytes each fail on the third with the log byte quota refusal. Exactly two checkpoints are recorded, and left uncaught the failure ends the run as `Quota` before the block's later `return` runs. With a log event quota of 1, a shared library that calls `log('one')` and then `log('two')` fails on the second call, which left uncaught ends the run with `lua log event quota exceeded`.

## Model reply size and wait time

Two limits guard every model call. Both apply to every round in the run, [`models.infer`](10-models.md#running-a-round-with-modelsinfer) rounds (nested ones included) and [`models.loop`](11-conversations.md#a-first-conversation) rounds alike, so an oversized or stalled reply ends that round with an error instead of hanging the run. Only the host changes either one.

### The response cap

A model reply may be up to 16 MiB (16,777,216 bytes) by default. A reply that would pass the response cap is refused as its bytes arrive, before any decoding, and the cap covers error replies as well as successful ones. The call fails with a malformed-response error whose message names the byte limit:

````text
malformed response: response stream exceeds the {max_bytes}-byte limit
````

An error reply over the cap names the limit the same way, as `response body exceeds the {cap}-byte limit` or `response body of {len} bytes exceeds the {cap}-byte limit`.

### The receive timeout

Every model call has a receive timeout of 120 seconds by default. The call waits at most that long for the reply headers, and then at most that long for each next chunk of the body. Every arriving chunk restarts the wait, and there is no limit on the whole request, so a long reply that keeps streaming is never cut off. A reply that stalls fails the call as a transport failure:

````text
http transport failure
````

Both failures are model call failures: kind `internal` when caught, run error kind `Completion` when uncaught, and both count as transient, so the run may succeed when run again ([Model call and environment failures](#model-call-and-environment-failures)).

## Parse error kinds

A parse failure stops the prompt before any section runs, and every parse failure has exactly one of five parse error kinds. The frontmatter is decoded before the body is checked, so a frontmatter failure is reported ahead of any body failure, and the kind tells you which part of the file to look at:

| Parse error kind | Raised when |
|---|---|
| `Frontmatter` | the frontmatter block at the top of the file is missing, unclosed, or not valid YAML, or the prompt contract rejects a value in it |
| `Structure` | the H1 title or the heading tree is invalid, or the file has no `promptforge:` key when it is run |
| `Fence` | a `lua shared` fence is repeated or outside the H1 body, or a fence is left unclosed |
| `List` | a list section holds something other than list items, an empty item, or no items |
| `Lua` | a Lua block does not compile |

Common mistakes land in predictable kinds: malformed YAML and an out-of-range `max_tool_iterations` give `Frontmatter`, duplicate sibling headings give `Structure`, an empty list item gives `List`, and a Lua syntax error gives `Lua`. Whatever its parse error kind, a parse failure ends the run with run error kind `Parse`.

### Frontmatter

`Frontmatter` covers the YAML block itself and every value the prompt contract checks ([Frontmatter rules and errors](02-file-structure.md#frontmatter-rules-and-errors)). YAML that does not parse gives this message, where `{message}` is a readable YAML diagnostic that never dumps the raw source:

````text
invalid frontmatter: {message}
````

A value the contract rejects, such as a malformed capability id or an out-of-range `max_tool_iterations`, gives that key's own message.

### Structure

`Structure` covers the H1 title and the heading tree ([Sections and nesting](02-file-structure.md#sections-and-nesting)):

````text
prompt requires an H1 title
prompt must contain exactly one H1 title
prompt H1 title must not be empty
section `{name}` is an orphan H{level} heading with no parent H{n}
an H{level} section heading must not be empty
duplicate sibling section name `{name}`: first declared at line {first_line}, again at line {line}; sibling section names must be unique
````

The first three mean the H1 title is missing, appears more than once, or is empty. The orphan message means a heading has no parent one level up, the next means a heading is empty, and the last means two sibling sections share a name.

A file whose frontmatter has no `promptforge:` key parses, but its run is refused on the first step with this `Structure` message ([The promptforge version](02-file-structure.md#the-promptforge-version)):

````text
not a promptforge prompt: no promptforge version
````

A prompt with an H1 title and no `##` sections is valid and runs.

### Fence

`Fence` covers where `lua shared` fences go and whether fences are closed ([Writing a Lua fence](03-blocks-and-prose.md#writing-a-lua-fence)):

````text
prompt allows at most one `lua shared` fence
`lua shared` fence is allowed only in H1
{label} fence is not closed
section `{name}` `lua` fence is not closed exactly
````

### List

`List` covers the contents of a list section ([List sections](03-blocks-and-prose.md#list-sections)):

````text
section `{name}` is a list section but contains non-list content: {line}
empty bullet item in list section `{name}`
section `{name}` is a list section but has no items
````

### Lua

Every Lua block, whether the shared library, an H1 block, or a section block, is compiled when the file is parsed. A syntax error fails the parse with parse error kind `Lua` and a message naming the section and block plus the Lua compiler's diagnostic. [Error locations in the prompt file](05-lua-environment.md#error-locations-in-the-prompt-file) shows the message layout.

## Finding where a parse failed

A parse failure comes with a location when the parser can point at the problem. The host reports it beside the message as a path, a line, a column, and a byte span, each when known. Lines are 1-based and count from the top of the file, the frontmatter included, with the opening `---` as line 1.

### Frontmatter failures

A frontmatter failure, whether the YAML is invalid or the contract rejects a value, gives a 1-based line and a 1-based column. For a capability entry on line 5 whose value is not a capability id, the failure reports line 5 and column 5, where the value starts after the `  - ` list marker.

A frontmatter failure has no prompt name, because the name comes from the frontmatter itself. Its location path is the placeholder `<prompt>`, and the host may label the failure with its own name for the file instead.

### Body failures

A `Structure`, `Fence`, or `List` failure holds the prompt's frontmatter `name`. When the parser can point at the offending region, such as a duplicate sibling section, it also gives the 1-based file line and the 1-based column where that region starts; body failures without a located region give neither. The column counts bytes, so a multibyte UTF-8 character earlier on the line pushes it past the character count. The message is the plain diagnostic with no added prefix, and the name, line, and column are separate details beside it.

This file names two sibling sections `S`:

````markdown
---
name: dup
description: Two sibling sections share a name
promptforge: 0
---

# T

## S

First.

## S

Second.
````

It fails the parse with parse error kind `Structure` and this message:

````text
duplicate sibling section name `S`: first declared at line 9, again at line 13; sibling section names must be unique
````

The location path is `dup`, the line is 13, and the column is 1. The run error kind is `Parse`.

### Lua compile failures

A Lua syntax error's position is inside its message: the Lua compiler's diagnostic passes through word for word, and the separate name, line, and column details are empty. [Error locations in the prompt file](05-lua-environment.md#error-locations-in-the-prompt-file) shows how to read the position.

### Failures after the parse

Only parse failures have a prompt location. An `Internal` failure names an engine source file and line instead, and every other run error kind reports no location. For the prompt line a runtime Lua error names in its message, see [Error locations in the prompt file](05-lua-environment.md#error-locations-in-the-prompt-file).

## Errors caught in Lua

An error value is the table `pcall` returns for a failure, with its error kind in `err.kind` and its text in `err.message` ([Catching and inspecting errors](05-lua-environment.md#catching-and-inspecting-errors)). Most error kinds belong to one feature and are taught with it. Three are families that gather failures from many places: `lua`, `internal`, and `cancelled`.

### The lua family

`kind == 'lua'` covers every failure on the Lua side of a run as one family:

- a Lua runtime error, your own `error` and `assert` calls included
- a built-in call's own argument error
- running past the memory ceiling
- a refused `log` call
- a failed `{{ }}` substitution ([Substitution errors](07-substitution.md#substitution-errors))
- an ordinary failed `store` call ([Store errors](09-the-store.md#store-errors))

The message text is what tells them apart.

### The internal family

`kind == 'internal'` marks a failure the prompt cannot fix:

- a model call failure: an HTTP transport failure (a receive timeout included), a backend error status, a malformed reply (an oversized one included), a missing or invalid environment variable, invalid client configuration, or a disabled gateway
- the missing-model error, raised when a model round has no model selected ([Choosing a section's model](10-models.md#choosing-a-sections-model))
- a failure of the host's input source for `user_input` ([Asking the operator with user_input](05-lua-environment.md#asking-the-operator-with-user_input))
- a fault in the engine or in the Lua runtime's own machinery

An ordinary failed `store` call is kind `lua`, not `internal`.

### The cancelled kind

`kind == 'cancelled'` covers a host cancel and a cancelled task. For a host cancel the message is `interrupted by Ctrl-C` ([Calls waiting during a cancel](#calls-waiting-during-a-cancel)). For a cancelled task, `err.task` holds the task id ([Cancellation and task lifetimes](15-tasks.md#cancellation-and-task-lifetimes)).

### Error kinds and run error kinds

An error kind is what Lua sees at the call. A run error kind is what the host reports when a failure ends the run. They are separate vocabularies, and one error kind can lead to different run error kinds depending on its source. An uncaught failure ends the run with its kind, message, and fields intact, never flattened to a message string, so the run is classified the same as the original error: an uncaught `context_exhausted` ends the run as `ContextExhausted` with a message starting `context exhausted: `.

| `err.kind` | Its own fields | Uncaught, ends the run as |
|---|---|---|
| `lua` | none | `Lua`, or `Quota` for a refused `log` call |
| `internal` | none | `Completion` for a model call failure, `Binding` for the missing-model error, `Input` for a `user_input` failure, `Internal` for an engine fault |
| `cancelled` | `task`, for a cancelled task | the cancelled outcome for a host cancel; `Lua` for a cancelled task's error value raised again right after its wait |
| `context_exhausted` | `reason` | `ContextExhausted` |
| `empty_model_reply` | `finish_reason`, only when the backend gave one | `Completion` |
| `tool_loop_exhausted` | none | `Tool` |
| `tool` | none | `Tool` |
| `out_of_scope_tool` | `name` | `Tool` |
| `unbound_tool` | `name` | `Tool` |
| `tasks_live` | `tasks` | `Lua` |
| `task_not_owned` | `task` | `Lua` |
| `task_consumed` | `task` | `Lua` |

Every other error value has only `kind` and `message`. In the H1 pass, some of these failures end the run as `RequirementsUnmet` instead, as [How a failed run is classified](#how-a-failed-run-is-classified) explains. An error value raised inside a local tool handler reaches a `pcall` around the call with its `kind` kept ([Local tools](12-tools.md#local-tools)).

### Raising a caught error again

When you catch an error value and raise it again with `error(err)`, the run's classification depends on when you raise it. Raised again before any other [suspending call](05-lua-environment.md#calls-that-wait-and-errors-that-raise), it ends the run exactly as if it had never been caught, with the same run error kind. Raised again later, after another suspending call, the failure is rebuilt from the value's `kind`:

| `err.kind` | Raised again later, ends the run as |
|---|---|
| `tool_loop_exhausted`, `tool` | `Tool` |
| `empty_model_reply` | `Completion` |
| `context_exhausted`, with its `reason` | `ContextExhausted` |
| `cancelled` | the cancelled outcome, not a failed run |
| `task_not_owned`, `task_consumed`, with their `task` | `Lua`, in the H1 pass too |
| `out_of_scope_tool`, `unbound_tool`, `tasks_live`, `lua`, `internal` | `Lua`, or `RequirementsUnmet` in the H1 pass |

A value missing its kind's fields also ends the run as `Lua`, or as `RequirementsUnmet` in the H1 pass.

## How a failed run is classified

A failed run reports exactly one run error kind. The kind names what failed, and the message beside it is always the underlying error's own text with its full cause chain; the kind never replaces it. There are thirteen run error kinds. A host cancel is not among them, because a cancelled run has not failed.

| Run error kind | The run failed because | Message |
|---|---|---|
| `Parse` | the file failed to parse, or has no `promptforge:` key | the parse failure's own message |
| `Version` | `promptforge:` declares a major version other than `0` | `unsupported promptforge version: {n} (this build supports major 0)` |
| `Binding` | a model round had no model selected | `model binding required for section {section}` |
| `Completion` | a model call failed, or an empty reply went uncaught | the call's own message, such as `non-success backend status {status}` |
| `Tool` | a tool call failed, was out of scope, or named an unbound tool, or a `models.loop` call reached the round cap | `tool call failure: {message}`, or one of the other tool messages below |
| `Store` | the host's store backend failed | `store operation failed` |
| `Determinism` | two live chains claimed one store path in conflicting ways | `store determinism violation: {detail}` |
| `Lua` | Lua failed at run time or returned an unusable value | the Lua error's message |
| `Quota` | a section VM ran out a log quota | `lua log event quota exceeded` or `lua log byte quota exceeded` |
| `ContextExhausted` | the compactor ran out of the model's context window | `context exhausted: {reason}` |
| `Input` | the host's input source failed a `user_input` request | `user input request was not answered: {message}` |
| `Internal` | an engine invariant broke | `internal invariant violated: {message}` |
| `RequirementsUnmet` | prepare refused the run, or the H1 pass failed its hard gate | the requirements notice, or the Lua error text |

Nothing reruns a failed run automatically. [Model call and environment failures](#model-call-and-environment-failures) lists the failures worth running again.

### Before the run starts

- `Parse`: every parse failure, a Lua syntax error included, is reported as a failed run with run error kind `Parse`, while its parse error kind says which part of the file failed ([Parse error kinds](#parse-error-kinds)). A file with no `promptforge:` key also ends its run on the first step with `Parse`.
- `Version`: the `promptforge:` key declares a major version other than `0`, and the run ends on its first step ([The promptforge version](02-file-structure.md#the-promptforge-version)).
- `RequirementsUnmet` at prepare: a required capability is missing or fails to activate, two declared capabilities conflict, a tool slot names a capability that contributed no tools, or a filled model fails a hard requirement, so prepare refuses the run before it starts, and the message is the requirements notice ([When a run cannot start](04-how-a-prompt-runs.md#when-a-run-cannot-start)).

### Model, tool, and input failures

- `Binding`: a section sends prose to a model, or calls `models.infer` without a handle, while neither `models.use` nor a prompt-wide `models.default` is in effect ([Choosing a section's model](10-models.md#choosing-a-sections-model)). Caught with `pcall`, the same error is kind `internal`. The kind also covers a host tool whose schema the host cannot offer to the model, which nothing in a prompt causes.
- `Completion`: a model call fails at the transport, backend, or decode layer and the prompt does not catch it, missing or invalid environment variables, invalid client configuration, and a disabled gateway included; or an `empty_model_reply` goes uncaught ([Empty and truncated replies](11-conversations.md#empty-and-truncated-replies)).
- `Tool`: a dispatched tool fails ([Tool failures](12-tools.md#tool-failures)), a tool call is out of the section's scope ([Advertising tools to the model](12-tools.md#advertising-tools-to-the-model)), a call names a tool not bound in the run ([Calling tools from Lua](12-tools.md#calling-tools-from-lua)), or a `models.loop` call does not finish within the round cap ([The round cap](11-conversations.md#the-round-cap)), and the prompt does not catch it.
- `ContextExhausted`: the selected compactor runs out of the model's context window and the prompt does not catch it ([Compactors and context exhaustion](11-conversations.md#compactors-and-context-exhaustion)).
- `Input`: the host's input source fails a `user_input` request and the failure goes uncaught. Caught with `pcall`, the same failure is kind `internal`.

The four `Tool` messages are these, where `{name:?}` and the other `:?` placeholders show the value in double quotes with escapes:

````text
tool call failure: {message}
tool {name:?} is not in this section's scope; in-scope aliases: {in_scope:?}
tool {name:?} is not bound in this run; bound aliases: {bound:?}
tool-call loop did not converge
````

### Lua and quota failures

- `Lua`: a section's Lua fails at run time or does not return a usable value. That covers an uncaught runtime error, running past the memory ceiling, a failed `store` call, a failed substitution, a misused task (an uncaught `tasks_live`, `task_not_owned`, or `task_consumed`, see [Task errors](15-tasks.md#task-errors)), and a block that returns a table ([Block and section returns](04-how-a-prompt-runs.md#block-and-section-returns)). It holds the same way in a walked section, a `call` chain, a task, and a fanout arm; in the H1 pass the ordinary Lua errors among them end the run as `RequirementsUnmet` instead, as the H1 pass hard gate below explains.
- A failed `{{ }}` substitution ends the run as `Lua` with the substitution's own message, or as `RequirementsUnmet` in the H1 pass. Substitution has no run error kind of its own.
- `Quota`: a section VM runs out the log event quota or the log byte quota. Only the two log quotas lead to `Quota`: running past the memory ceiling is `Lua`, and no instruction count can run out.

### Store and engine failures

- An author's own failed `store` call is an ordinary `lua`-kind error value, and `Lua` when uncaught ([Store errors](09-the-store.md#store-errors)).
- `Determinism`: two live chains claim the same store path in conflicting ways ([Sharing the store across calls and tasks](09-the-store.md#sharing-the-store-across-calls-and-tasks)). In block code the run ends on the spot: the store call never returns into Lua, so no `pcall` can catch it. The message names the path, both chains, and both claim kinds.
- Only while the `lua shared` fence loads does a claims conflict raise at the call instead, as a `lua`-kind error value with this message:

````text
write-write race on {path}: another live identity holds a claim on it
````

- `Store`: appears only when the host's store backend itself fails outside any store call, as the run starts or as the store is opened for the H1 pass, the section walk, or a new task. Nothing in a prompt causes it.
- `Internal`: an engine invariant broke, a fault in the engine rather than a mistake in the prompt. Its location names an engine source file and line.

### The H1 pass hard gate

The [H1 pass](04-how-a-prompt-runs.md#the-h1-pass) is a hard gate. An uncaught failure there that would otherwise end the run as `Lua` ends it as `RequirementsUnmet` instead, with the Lua error text as the requirements notice, and later H1 blocks and every section never run:

````markdown
---
name: gated
description: Stops before the walk when its gate fails
promptforge: 0
---

# Gated

```lua
assert(false, 'the gate cannot hold')
```

## Work

```lua
return 'never reached'
```
````

The run fails with run error kind `RequirementsUnmet` and a message that contains `the gate cannot hold`, and `## Work` never runs. Move the same `assert` into a block under `## Work` and the run fails as `Lua` instead.

In the H1 pass, these failures become `RequirementsUnmet`:

- an `error` or `assert` call, or any other runtime fault, in a block in the H1 body
- a failed substitution
- running past the memory ceiling
- an ordinary failed `store` call
- an error value raised again after another suspending call that is rebuilt as `Lua`: kind `out_of_scope_tool`, `unbound_tool`, `tasks_live`, `lua`, or `internal`, or a value missing its fields

Everything else keeps its own classification in the H1 pass:

- Task errors stay `Lua`: an uncaught `tasks_live`, `task_not_owned`, or `task_consumed`, a delivered cancelled task's error value, and a `task_not_owned` or `task_consumed` value raised again later with its `task` field.
- Tool failures stay `Tool`, and `Quota`, `Completion`, `Binding`, `ContextExhausted`, and `Input` keep their kinds.
- A claims conflict stays `Determinism`.
- A host cancel stays the cancelled outcome.
- A failure while the shared library loads, a failure in the `var` read-back, and a bad `jump` target from the H1 pass stay `Lua`.

## Model call and environment failures

A model call that fails on the host's side reaches Lua as an error value of kind `internal` with no extra fields. Its message never includes the reply body, so a hostile or private payload cannot leak into a message or forge a log line:

| Failure | Message |
|---|---|
| connection failure or receive timeout | `http transport failure` |
| non-success status from the gateway | `non-success backend status {status}` |
| oversized or undecodable reply | `malformed response: {message}` |
| error reply whose body cannot be read | `unreadable backend error body (status {status})` |
| gateway disabled by the host | `gateway access is disabled` |
| environment variable not set | `missing environment variable: {name}` |
| environment variable not valid Unicode | `environment variable is set but not valid Unicode: {name}` |

A `models.loop` round answered with HTTP 500 raises `non-success backend status 500`, and a 502 whose body holds forged log text still shows only `502`. Left uncaught, every one of these ends the run as `Completion`, the environment, configuration, and gateway failures included; none of them has a separate setup kind.

Catch them like any other error value:

````lua
local ok, reply = pcall(models.infer, prose)
if not ok then
  if reply.kind == 'internal' then
    log('model call failed')
    return 'skipped: ' .. reply.message
  end
  error(reply)
end
return reply
````

A backend answering with status 503 makes this block return `skipped: non-success backend status 503`. The missing-model error is kind `internal` too, so the block also returns a `skipped: ` result in a section with no model selected.

### Environment variables

The host's model connection reads two environment variables. A prompt never reads them; it only sees the error when one is missing or invalid.

- `PROMPTFORGE_GATEWAY_URL` is always required.
- `PROMPTFORGE_GATEWAY_API_KEY` is required unless the URL's host is loopback: `127.0.0.1`, `::1`, or `localhost`. An empty key counts as unset.

A run that needs an unset variable fails with a message naming it, and one set to a value that is not valid Unicode fails with the distinct message in the table above.

### Failures worth running again

Nothing reruns a failed run or a failed model call automatically. A run that failed on a transient model call problem may succeed when run again, and these count as transient:

- transport failures, a receive timeout included
- malformed or oversized replies
- unreadable backend error bodies
- backend statuses of 500 or higher

A status below 500 is not transient, and neither is any other failure.

## Cancelling a run

A host cancel stops a run from outside, for example when the person running it presses Ctrl-C. A prompt cannot cancel its own run; it only observes the outcome. The run ends with the cancelled outcome, a clean stop rather than a failure, so it has no run error kind.

A cancel reaches running Lua promptly, even a tight endless loop:

````markdown
---
name: spinner
description: Loops until the host cancels the run
promptforge: 0
---

# Spinner

## Loop

```lua
local n = 0
while true do n = n + 1 end
```
````

Nothing in this prompt ends the loop, and no limit does either. When the host cancels, the loop is aborted and the run ends with the cancelled outcome.

### How a cancel reaches running Lua

- Every 10,000 Lua instructions, running Lua checks the run's cancel flag. The check covers each section VM's main code and every block coroutine, so it reaches every block of every section, the H1 pass included. The engine also checks the flag between steps.
- Every section VM and every activated capability share the same cancel flag.
- Once the flag is set, the running block fails with the interrupted error: kind `cancelled`, message `interrupted by Ctrl-C` whatever the host's actual trigger was, and no source location. It never appears as an ordinary Lua runtime error, and it wins over any error value the block had raised.
- The cancel stops every block in the run, not only the first. After a cancel, a block with a bounded loop such as `for i = 1, 100000 do end` followed by `return "done"` never returns `done`.
- A running `models.loop` stays cancellable, because the loop runs as Lua inside your block and the check keeps running while it does.

Left uncaught, the interrupted error ends the run with the cancelled outcome.

## Calls waiting during a cancel

A cancel also reaches every call that is waiting on the host. Each one resumes with the interrupted error, kind `cancelled`, message `interrupted by Ctrl-C`:

- `models.infer`, and each `models.loop` round
- `tools.call`, and tool calls the model makes
- `user_input()`
- every `store` call
- a `tasks.events` read ([Read options, results, and errors](16-task-events.md#read-options-results-and-errors)), with the model's `task_events` built-in failing the same way

Calls made inside a local tool handler are ordinary suspending calls and resume the same way ([Local tools](12-tools.md#local-tools)).

`pcall` catches the interrupted error at the call like any other error value. With a tool slot `search` bound:

````lua
local ok, out = pcall(tools.call, 'search', { query = args })
if not ok then
  if out.kind == 'cancelled' then
    log('stopped while searching')
  end
  error(out)
end
return out
````

A cancel during the tool call gives `ok == false`, `out.kind == 'cancelled'`, and `tostring(out) == 'interrupted by Ctrl-C'`. A `models.loop` round cut short the same way raises an error value whose `kind` is `cancelled` and whose `message` is `interrupted by Ctrl-C`, so `tostring(err)` gives exactly that message.

Catching a cancel does not keep the run going. Once the host cancels, running Lua is stopped by the instruction check and the engine's next step tears every chain down, so the run still ends with the cancelled outcome. Raise the caught value again, as above, rather than trying to continue.

### Work in flight

- A tool call in flight is interrupted rather than waited out, and the run ends promptly with the cancelled outcome, even when the tool itself would never finish.
- A `models.infer` round in flight is aborted before its reply lands, and no `model_turn_failed` event is reported for it; a round that really fails does report one ([Model round events](16-task-events.md#model-round-events)).
- A cancel that lands while a block waits on a `store.write` ends the run with the cancelled outcome the same way.
- A run cancelled before a [`fanout`](14-fanout.md#the-fanout-call) begins ends with the cancelled outcome without running any arms.
