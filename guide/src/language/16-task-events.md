# Task Events

A run reports everything it does as it goes: each section that starts and finishes, each Lua block, each model round with its token counts, each tool call and store operation, and each task that starts and ends. `tasks.events` hands those reports back to your Lua code as plain tables, so a prompt can check what a task actually did, add up what its model calls cost, or follow a long task while it is still running. This chapter shows you how to read a task's history, what every kind of event means and holds, and which events a prompt never gets back.

## Reading a task's history

An event is one report the run makes as it works, such as a section starting, a store write finishing, or a model round ending. The run reports events at every boundary of the parse and the run: the parse itself, the run, each section, each Lua block, each model round, each tool call, each store operation, each wait for operator input, and each task. The host keeps every event in its run log, and `tasks.events(task)` reads one task's history, the events that task has reported so far:

````markdown
---
name: child-history
description: Starts a task and lists the events it reported
promptforge: 0
---

# Child history

## Start

```lua
local t = tasks.spawn('### Child')
tasks.when_any({ t })
local kinds = {}
for _, e in ipairs(tasks.events(t)) do
  kinds[#kinds + 1] = e.kind
end
return table.concat(kinds, '\n')
```

### Child

```lua
return 'done'
```
````

`tasks.spawn('### Child')` starts the child section as a [task](15-tasks.md#starting-a-task) and returns its Task handle, and [`tasks.when_any({ t })`](15-tasks.md#waiting-for-results) waits until that task ends. `tasks.events(t)` then returns the task's history as a 1-based Lua sequence of plain tables, one per event, in the order the task reported them. The host serves the read from its run log.

Each table's `kind` field names its event, so the block returns one line per event. The first line is `section_started`, reported when `### Child` began, and the last is `task_succeeded`, reported when the task ended with a result. The lines between report the child's section VM starting up, its Lua block running, and its section finishing.

### What an event holds

Every event table holds the same four keys, followed by the fields of its own kind, if it has any. A successful store write reads like this in Lua:

````lua
{
  kind = 'store_write_succeeded',
  execution = 'run-1',
  section = 'Gather',
  provenance = { task = '0.2', seq = 9 },
}
````

- `kind` names the event.
- `execution` identifies the run.
- `section` is the heading text of the section that reported the event.
- `provenance` says which task reported the event, in `provenance.task`, and where the event falls in that task's count, in `provenance.seq`.

`execution`, `section`, and `provenance` are the event's three coordinates. An event that holds only `kind` and the three coordinates, such as `run_started`, `section_finished`, or `store_write_succeeded`, is a boundary event: it marks the moment something began, ended, or failed, and says nothing more. Of the 57 kinds, 42 are boundary events, and the other 15 add fields of their own, which this chapter gives with each kind. Read every field with ordinary indexing, as in `e.kind`, `e.section`, `e.provenance.task`, and `e.provenance.seq`.

### Event kinds

A kind is the event's name written in snake_case, such as `run_started`, `section_finished`, or `store_read_numbered_succeeded`. Here are all 57, by area:

| Area | Kinds | Covered in |
|---|---|---|
| Parse | `parse_started`, `parse_succeeded`, `parse_failed` | [Lua block and parse events](#lua-block-and-parse-events) |
| Run | `run_started`, `run_succeeded`, `run_failed` | [Run and section boundaries](#run-and-section-boundaries) |
| Section | `section_started`, `section_finished` | [Run and section boundaries](#run-and-section-boundaries) |
| Section VM | `lua_shared_load_started`, `lua_shared_load_succeeded`, `lua_shared_load_failed`, `lua_chunk_started`, `lua_chunk_succeeded`, `lua_chunk_failed`, `lua_teardown_started`, `lua_teardown_succeeded` | [Lua block and parse events](#lua-block-and-parse-events) |
| Compiling at parse time | `lua_compilation_started`, `lua_compilation_succeeded`, `lua_compilation_failed` | [Lua block and parse events](#lua-block-and-parse-events) |
| Author checkpoint | `lua` | [Lua block and parse events](#lua-block-and-parse-events) |
| Model | `model_turn_completed`, `model_turn_failed`, `model_turn_truncated`, `model_metadata_degraded`, `thinking`, `assistant_reply`, `assistant_tool_calls` | [Model round events](#model-round-events) |
| Tools | `tool_scope_validation_started`, `tool_scope_validation_succeeded`, `tool_scope_validation_failed`, `tool_call_succeeded`, `tool_call_failed`, `tool_result` | [Tool call events](#tool-call-events) |
| Store | `store_write_succeeded`, `store_write_failed`, `store_append_succeeded`, `store_append_failed`, `store_read_succeeded`, `store_read_failed`, `store_read_numbered_succeeded`, `store_read_numbered_failed`, `store_replace_succeeded`, `store_replace_failed`, `store_delete_succeeded`, `store_delete_failed`, `store_glob_succeeded`, `store_glob_failed` | [Store and operator input events](#store-and-operator-input-events) |
| Operator input | `user_input_wait_started`, `user_input` | [Store and operator input events](#store-and-operator-input-events) |
| Tasks | `task_started`, `task_succeeded`, `task_failed`, `task_cancelled`, `task_abandoned`, `task_notice` | [Task lifecycle events](#task-lifecycle-events) |
| Debug capture, only when the host switches it on | `request`, `response` | [Model round events](#model-round-events) |

### The section label

`section` is heading text without the `#` markers, so `## Inner` reports as `Inner`. Every event a section's code causes has that section's heading, and a section reached through [`call`](08-jump-and-call.md#called-chains) reports under its own heading, not its caller's. The [H1 pass](04-how-a-prompt-runs.md#the-h1-pass) and the run's opening and closing events report under the H1 title, and parse events report under `Prompt`. Whatever heading you write becomes the label on every event its section reports, so headings you can recognize make a history easy to read.

### Which tasks a chain can read

A chain can read exactly two kinds of task: a task it [owns](15-tasks.md#starting-a-task), such as one it started with `tasks.spawn`, and the task it runs inside, whose id is [`sys.taskid`](05-lua-environment.md#run-metadata-in-sys). This is the same owner-or-self rule that [`tasks.status`](15-tasks.md#checking-on-tasks) follows. Pass `sys.taskid` to read the current task's own history, inside a spawned task or on the main walk, which reads itself as task `0`:

````lua
local mine = tasks.events(sys.taskid)
````

On the main walk `#mine` is always above 0, because `run_started` opens the run's events. The timers that timed waits use behind the scenes are never readable tasks.

### Reading only new events

A read returns what the task has reported so far, and anything reported later needs another read. Pass `{ last = seq }` as the second argument to get only the events reported after `seq`, the highest `provenance.seq` you have already seen. `last` is exclusive, so a polling loop that passes back the highest `seq` it has seen reads each event exactly once:

````markdown
---
name: progress
description: Follows a task's events while it runs
promptforge: 0
---

# Progress

## Watch

```lua
local t = tasks.spawn('### Work')
local seen
local kinds = {}
repeat
  local ended = tasks.when_any({ t }, { timeout = 2 })
  for _, e in ipairs(tasks.events(t, { last = seen })) do
    seen = e.provenance.seq
    kinds[#kinds + 1] = e.kind
  end
until ended
return table.concat(kinds, '\n')
```

### Work

```lua
store.write('draft.md', 'outline')
store.append('draft.md', '\nbody')
return 'done'
```
````

With a `timeout`, `tasks.when_any` returns `nil` if the task is still running after 2 seconds, and the task's handle once it has ended, as [Time limits on waits](15-tasks.md#time-limits-on-waits) describes. Each iteration reads only what the task reported since the previous one and keeps the newest `seq` in `seen`. On the first iteration `seen` is nil, and a `last` of nil reads from the task's first event. The result lists every event of `### Work` once, however many iterations the loop takes.

## Read options, results, and errors

Name the task with a Task handle, as in `tasks.events(t)`, or with a bare [task id](15-tasks.md#task-handles-and-ids) string such as `'0'` or `'0.2'`. The second argument is optional. When you pass it, it is a table whose only option is `last`, a whole number from 0 to 4294967295, which is the full range of `provenance.seq`. A float with no fractional part counts as that whole number, so `last = 3.0` reads as `3`. With no second argument, or no `last` in it, the read starts at the task's first event.

The result is always a sequence table. When nothing new has been reported it is empty, with `#events == 0`, so `#` and `ipairs` work on any result without a nil check.

An optional field that an event leaves out reads as `nil`, never as a placeholder value, so a plain truth test checks for it. That covers an `assistant_reply` event's `finish_reason` when the provider sent no stop label, its `metrics` when nothing was measured, and any seed a task started without:

````lua
local reason = e.finish_reason or 'no stop label'
````

Reading a history has no side effects: a `tasks.events` call adds no events of its own. A task that reads its own history sees every event reported before the read.

The model reads the same host-kept history with its `task_events` built-in, as [The model's status, cancel, and history tools](15-tasks.md#the-models-status-cancel-and-history-tools) describes. Your Lua code gets the events back as a sequence of tables, while the same read made by the model comes back to the model as untrusted text.

### Read errors

`tasks.events` checks its arguments at the call, and a broken rule raises an [error value](05-lua-environment.md#catching-and-inspecting-errors) there, where `pcall` catches it. `err.kind` and `err.message` read as for any error value, and `tostring(err)` returns the message:

| Kind | Raised when | Message |
|---|---|---|
| `lua` | the second argument is not a table | `tasks.events opts must be a table, got {type}` |
| `lua` | `last` is not a number | `tasks.events last must be a number, got {type}` |
| `lua` | the task argument is neither a Task handle nor a string | `tasks.events expects a Task handle or task id, got {type}` |
| `lua` | the task string is not a dot-separated task id | `` `{text}` is not a task id: required a dot-separated path such as `0.1` `` |
| `lua` | `last` is negative, fractional, or above 4294967295 | `last must be a non-negative integer sequence number, got {value}` |
| `task_not_owned` | the chain neither owns the task nor runs inside it, including an id that names no task | `` task `{task}` is not a task this chain owns ``, with the id in the error's `task` field |

When one call breaks several rules, the first broken rule in this order is reported: the types of the options table and `last`, then the task argument and the range of `last`, and ownership last. An id that names no task at all gets the same `task_not_owned` refusal as a task another chain owns, so a chain learns nothing about tasks it never started. When a task id reaches your code from somewhere other than your own `tasks.spawn`, such as the argument string, guard the read:

````lua
local ok, history = pcall(tasks.events, args)
if not ok then
  if history.kind == 'task_not_owned' then
    return 'not my task: ' .. history.task
  end
  error(history)
end
return #history .. ' events so far'
````

If the run is cancelled while a read is still waiting for the host, the read fails with an error value of kind `cancelled`, and the model's `task_events` read fails the same way. [Cancelling a run](17-limits-and-errors.md#cancelling-a-run) covers what a cancel does to the rest of the run.

## Event coordinates and task ids

Every event has three coordinates that trace it back to its run and its reporter:

- `execution` is a string that identifies the run. One value runs through every event of a run, from the parse to the end.
- `section` is the heading text of the section that reported the event. A [`models.loop`](11-conversations.md#a-first-conversation) conversation's events report under the section that ran the loop.
- `provenance` names the task that reported the event, in `task`, and the event's place in that task's count, in `seq`.

### Task ids and called sections

`provenance.task` is the id of the nearest task the event belongs to. Task ids are dot-separated paths: the main walk is task `0`, and the tasks it starts, such as the arms of a [`fanout`](14-fanout.md#the-fanout-call), are `0.0`, `0.1`, `0.2`, and so on. Each task stamps its own id on its events and keeps its own `seq` count.

A section reached through `call` is not a task of its own. It reports under its caller's task and continues the caller's `seq` numbers, because the caller waits for the `call` to finish and the two never interleave. This prompt shows where a called section's events land:

````markdown
---
name: call-report
description: Shows where a called section's events are filed
promptforge: 0
---

# Call report

## Outer

```lua
local reply = call('## Inner')
for _, e in ipairs(tasks.events(sys.taskid)) do
  if e.kind == 'lua' then
    return reply .. ' from ' .. e.section .. ' on task ' .. e.provenance.task
  end
end
```

## Inner

```lua
log('inner ran')
return 'hello'
```
````

[`log`](05-lua-environment.md#checkpoints-with-log) reports an event of kind `lua` whose `message` field holds the logged text. `## Outer` calls `## Inner`, then searches its own history for that event and returns:

````text
hello from Inner on task 0
````

The event has the called section's own heading, `Inner`, as its `section`, and the caller's task, `0`, as its `provenance.task`. A spawned task, by contrast, reports under its own id.

### Sequence numbers

`provenance.seq` strictly increases within a task, independently of every other task, so sorting a task's events by `seq` puts them in order without a clock. A spawned task counts from 0. The main walk's count starts after any parse events the host logged ahead of the run.

Event numbers can skip. One counter per task numbers both the [host work](01-what-a-prompt-is.md#the-prompt-and-its-host) the task asks for, such as model calls and store writes, and the events the task reports, so the two stay in order against each other. Each piece of host work takes the next number, so the numbers on events skip wherever the task asked the host for something. With no host work in between, event numbers run without gaps from the task's starting number. The counter itself never skips; only the events leave holes where host work took a number.

The pair of task id and `seq` is unique across a run's log. `seq` is a 32-bit unsigned count, from 0 to 4294967295, the same range `last` accepts.

### Reproducible ids

Two runs of the same prompt, given the same argument string and the same answers to their host work, get the same task ids and stamp the same `{ task, seq }` on the same events, however their chains interleave. Provenance sorts by task path first and by `seq` second.

### How the run log writes an event

Outside Lua, the host writes each event to its run log as one line of JSON: `kind` first, then `execution`, `section`, and `provenance`, then the kind's own fields. The store write shown earlier in this chapter becomes:

````text
{"kind":"store_write_succeeded","execution":"run-1","section":"Gather","provenance":{"task":"0.2","seq":9}}
````

An event table in Lua has the same keys and nesting.

## Run and section boundaries

A run's events open with `run_started` and close with `run_succeeded` or `run_failed`, both reported under the prompt's H1 title, with each walked section's events between them in document order. This prompt has two sections and a shared library:

````markdown
---
name: lifecycle
description: Two sections that share a helper
promptforge: 0
---

# Lifecycle

```lua shared
function shout(text)
  return string.upper(text)
end
```

## First

```lua
var.word = shout('hello')
```

## Second

```lua
return var.word
```
````

Leaving out any parse events the host logged first, the run log for this prompt reads as follows, one event per line with its `section` and then its `kind`:

````text
Lifecycle  run_started
First      section_started
First      lua_shared_load_started
First      lua_shared_load_succeeded
First      lua_chunk_started
First      lua_chunk_succeeded
First      lua_teardown_started
First      lua_teardown_succeeded
First      section_finished
Second     section_started
Second     lua_shared_load_started
Second     lua_shared_load_succeeded
Second     lua_chunk_started
Second     lua_chunk_succeeded
Second     lua_teardown_started
Second     lua_teardown_succeeded
Second     section_finished
Lifecycle  run_succeeded
````

Each section starts, its [section VM](03-blocks-and-prose.md#how-the-shared-library-loads) replays the shared library, runs the section's Lua block, and shuts down, and then the section finishes. `First` falls through, `Second` returns `HELLO` as the run result, and the run succeeds.

### What each boundary means

| Kind | Meaning |
|---|---|
| `parse_started` | Parsing of the prompt file began |
| `parse_succeeded` | Parsing, including compiling the prompt's Lua, completed |
| `parse_failed` | Parsing failed |
| `run_started` | The run passed its [version gate](02-file-structure.md#the-promptforge-version) and began |
| `run_succeeded` | The run returned a value |
| `run_failed` | The run returned an error |
| `section_started` | A walked section began |
| `section_finished` | A walked section completed successfully |

`run_started` is reported on the main walk, task `0`, under the H1 title and ahead of any section event, so the main walk can find it in `tasks.events(sys.taskid)`. A run ends with exactly one closing event, `run_succeeded` or `run_failed`, under the H1 title and after every task's terminal event. There is no run-cancelled kind: a [cancelled](04-how-a-prompt-runs.md#failure-and-cancellation) run also closes with `run_failed`, even though its outcome is cancelled rather than failed. A prompt never reads either closing event, because both are reported after every chain has stopped, so they appear only in the host's run log.

Each walked section reports `section_started` when it begins and `section_finished` only when it completes successfully, both with its heading text as `section`. A section completes by falling through, by [`jump`](08-jump-and-call.md#sibling-jumps), or by `return`, and it reports `section_finished` after its Lua teardown and before the next section reports `section_started`. The events of a task's chain have that task's id.

The H1 pass never reports `section_started` or `section_finished`, even when it ends with a return or an error, but it still reports its Lua teardown pair under the H1 title. A section still suspended when the run ends, waiting on a model, a tool, or a task, reports no `section_finished`.

### Spotting a failed section

There is no section-failed kind. A section whose chain ends in an error reports `section_started` but never `section_finished`, and that gap is how you find the failing section in a history. This block reports where a failed task stopped:

````lua
local t = tasks.spawn('### Risky')
local _, ok = tasks.when_any({ t })
if ok then
  return 'the task succeeded'
end
local open = {}
for _, e in ipairs(tasks.events(t)) do
  if e.kind == 'section_started' then open[e.section] = true end
  if e.kind == 'section_finished' then open[e.section] = nil end
end
for name in pairs(open) do
  return 'the task failed in ' .. name
end
return 'the task failed'
````

`tasks.when_any` returns `ok` as `false` when the task failed, and every section the task started but never finished is left in `open`.

## Lua block and parse events

### Section VM events

Every section VM reports its phases, with the section's heading as `section`:

- Replaying the shared library reports `lua_shared_load_started`, then `lua_shared_load_succeeded` or `lua_shared_load_failed`.
- Running each Lua block reports `lua_chunk_started`, then `lua_chunk_succeeded` or `lua_chunk_failed`.
- Teardown reports `lua_teardown_started` and `lua_teardown_succeeded`, back to back. Teardown has no failed kind.

When a block pauses at a [suspending call](05-lua-environment.md#calls-that-wait-and-errors-that-raise), such as a model call, a tool call, or a store operation, no event marks the pause. The block's `lua_chunk_started` and its closing event still bracket it, and the events of the host work it waited on come between them.

`lua_shared_load_failed` covers any error while the shared library loads or runs, including a call to `jump`, which is not available while the library loads. Every section reports exactly one teardown pair, even when its section VM fails before any block runs: a failing shared library reports `lua_shared_load_failed`, then `lua_teardown_started` and `lua_teardown_succeeded`.

The H1 pass reports its Lua block events under the H1 title. When its own Lua fails with an uncaught error, such as a failed `assert`, it reports `lua_chunk_failed` under the H1 title, and the run ends as [`RequirementsUnmet`](17-limits-and-errors.md#how-a-failed-run-is-classified) with the error's text as its notice, the hard gate that [The H1 pass](04-how-a-prompt-runs.md#the-h1-pass) describes.

A failed block's detail is the error it raised, never the event: `lua_chunk_failed` is a boundary event and holds only the coordinates.

### Checkpoints from log

Every `log(message)` call that passes its checks reports a `lua` event, your own checkpoint in the history. Its `message` field holds the text exactly as logged. Its `section` is the heading of the section whose Lua called `log`, which for a section reached through `call` is that section's own heading, and its `provenance` is the calling task. This section writes two checkpoints and reads them back:

````markdown
---
name: checkpoints
description: Reads back its own log checkpoints
promptforge: 0
---

# Checkpoints

## Work

```lua
log('loaded input')
store.write('notes.md', args)
log('saved notes')
local lines = {}
for _, e in ipairs(tasks.events(sys.taskid)) do
  if e.kind == 'lua' then
    lines[#lines + 1] = e.section .. ': ' .. e.message
  end
end
return table.concat(lines, '\n')
```
````

````text
Work: loaded input
Work: saved notes
````

A `log` call that breaks one of its rules raises at the call and reports nothing. The rules, from [Checkpoints with log](05-lua-environment.md#checkpoints-with-log), are exactly one argument, a UTF-8 string of at most 256 characters with no newline or control character, and a run that has not yet used up its log events or log bytes.

### Parse and compile events

`parse_started` and `parse_succeeded` bracket the parse of the prompt file, reported on task `0` with `Prompt` as their `section`. Between them, each Lua block compiled while the file is parsed reports `lua_compilation_started`, then `lua_compilation_succeeded` or `lua_compilation_failed`. Each block is compiled once, when the file is parsed, so these events belong to the parse, not to a section VM. A compilation event's `section` says where the Lua sits: the H1 title for Lua in the H1 body, and the section's heading for a section's Lua. No parse or compilation event includes Lua source or its location. In order, a parse reports:

1. `parse_started`
2. one compilation pair for each Lua block: `lua_compilation_started`, then `lua_compilation_succeeded` or `lua_compilation_failed`
3. `parse_succeeded` or `parse_failed`

The main walk sees these events in its own history only when the host logs parse events in the same log it serves reads from, as the standard runner does, and the main walk's own numbering then continues after them. A file whose parse fails never runs, so a prompt never reads `parse_failed`, or a `lua_compilation_failed`, which fails the parse. Both appear only in the host's run log.

## Store and operator input events

### Store events

Every [store](09-the-store.md#what-the-store-is) operation reports a succeeded or failed boundary event under the section that asked for it:

| Call | Kinds |
|---|---|
| `store.write` | `store_write_succeeded`, `store_write_failed` |
| `store.append` | `store_append_succeeded`, `store_append_failed` |
| `store.read` | `store_read_succeeded`, `store_read_failed` |
| `store.read_numbered` | `store_read_numbered_succeeded`, `store_read_numbered_failed` |
| `store.str_replace` | `store_replace_succeeded`, `store_replace_failed` |
| `store.delete` | `store_delete_succeeded`, `store_delete_failed` |
| `store.glob` | `store_glob_succeeded`, `store_glob_failed` |

Note that `store.str_replace` reports as `store_replace`, and that `store.exists` reports nothing. The store events hold no path, no content, and no error detail. A failed store call raises an error value of kind `lua` at the call, as [Store errors](09-the-store.md#store-errors) describes, and that error is where the detail lives. A store call made inside a [local tool](12-tools.md#local-tools) handler is an ordinary store operation and reports the same events.

A store operation's event is reported before the Lua call returns, so its outcome always comes before the block's closing event. Store and model work appear in the order the section did them: a write, then a read, then a model round report `store_write_succeeded`, then `store_read_succeeded`, then `model_turn_completed`, the event that ends a completed round. This prompt counts what a task did with the store:

````markdown
---
name: store-audit
description: Counts a task's store writes and failed reads
promptforge: 0
---

# Store audit

## Audit

```lua
local t = tasks.spawn('### Writer')
tasks.when_any({ t })
local writes, failed = 0, 0
for _, e in ipairs(tasks.events(t)) do
  if e.kind == 'store_write_succeeded' then writes = writes + 1 end
  if e.kind == 'store_read_failed' then failed = failed + 1 end
end
return 'writes=' .. writes .. ' failed=' .. failed
```

### Writer

```lua
store.write('a.md', 'alpha')
store.write('b.md', 'beta')
pcall(store.read, 'missing.md')
return 'done'
```
````

````text
writes=2 failed=1
````

The read of a file that does not exist raised an error value, which `pcall` caught inside the task, and the history still shows it as `store_read_failed`, with neither the path nor the error text.

### Operator input

A section that calls [`user_input()`](05-lua-environment.md#asking-the-operator-with-user_input) reports `user_input_wait_started` when it starts waiting for the operator. It is a single boundary with no finished counterpart, and it is reported on every wait, even in a host with no operator. When the operator's text arrives, a separate `user_input` event follows under the same section, and its `text` field holds that text exactly as the operator supplied it, byte for byte. The unavailable fallback reports no `user_input`, so counting both kinds tells you how many waits got no text:

````lua
local waits, answered = 0, 0
for _, e in ipairs(tasks.events(sys.taskid)) do
  if e.kind == 'user_input_wait_started' then waits = waits + 1 end
  if e.kind == 'user_input' then answered = answered + 1 end
end
local unanswered = waits - answered
````

`user_input.text` is untrusted.

## Model round events

Every [round](10-models.md#running-a-round-with-modelsinfer), from `models.infer` or from a `models.loop` conversation, reports its events under the section that ran it. This prompt makes one round and reads back the event that reports its reply:

````markdown
---
name: reply-audit
description: Reads back the reply event of its own round
promptforge: 0
models:
  writer: {}
---

# Reply audit

```lua
models.default('writer')
```

## Ask

```lua
local reply = models.infer('Name one prime number between 10 and 20.')
for _, e in ipairs(tasks.events(sys.taskid)) do
  if e.kind == 'assistant_reply' then
    return reply .. ' (' .. e.origin .. ', round ' .. e.turn .. ', ' .. e.model .. ')'
  end
end
```
````

The block returns a line such as `13 (infer, round 1, claude-sonnet-4-6)`. The `assistant_reply` event holds the same text that `models.infer` returned, along with `origin` `infer`, which marks a `models.infer` round, the round's number in `turn`, and the name of the model that served it.

### Round boundaries

Each round ends with `model_turn_completed` or `model_turn_failed`. A round whose text reply arrived but hit the model's length limit, with finish reason `length`, also reports `model_turn_truncated`, after `model_turn_completed`. An [empty reply](11-conversations.md#empty-and-truncated-replies) is a completed round, not a failed one. A provider's refusal for context length is a failed round, the case that [Compactors and context exhaustion](11-conversations.md#compactors-and-context-exhaustion) covers. A round's events are reported once the model's answer to that round arrives.

### Reply events

Each text reply reports an `assistant_reply` event. Beside the coordinates, it holds:

| Field | Value |
|---|---|
| `turn` | The round's number |
| `text` | The reply text |
| `finish_reason` | The provider's stop label, such as `"stop"` or `"length"`, or nil when the provider sent none |
| `model` | The name of the model that served the round, as the provider returned it |
| `origin` | `"infer"` for a `models.infer` round, `"chat"` for a `models.loop` round |
| `metrics` | What was measured about the call, or nil when nothing was |

The `model` name can differ from the bound model's id, since it is whatever the provider says served the round, and it is an empty string when the provider named no model. To tell whether the round you just made was cut off, check the latest reply's `finish_reason`:

````lua
models.use('writer', { max_tokens = 50 })
local text = models.infer('Describe the water cycle in detail.')
local latest
for _, e in ipairs(tasks.events(sys.taskid)) do
  if e.kind == 'assistant_reply' then latest = e end
end
if latest and latest.finish_reason == 'length' then
  return text .. ' [cut off]'
end
return text
````

`max_tokens`, a [sampling option](10-models.md#sampling-options), keeps this reply short, and a reply cut off at the model's length limit has finish reason `length`.

### Reasoning events

Each completed block of model reasoning reports a `thinking` event with `turn`, `model`, and `text`. It is reported only when the reasoning is present and non-empty, and it comes after `model_turn_completed` and before the round's reply event, in `models.loop` and `models.infer` rounds alike.

### Tool call batches

When the model answers with tool calls, the round reports an `assistant_tool_calls` event in place of `assistant_reply`, before any of the calls runs. Its fields are `turn`, `model`, and `calls`. Each call in `calls` has `id`, the provider-issued call id, `name`, and `arguments`, a table, exactly as the model produced them. That is the same `{ id, name, arguments }` shape that `models.loop` appends as the assistant record's `tool_calls`, as [What the loop appends](11-conversations.md#what-the-loop-appends) shows. The model calls a bound tool by its prompt-local alias, never by its tool path, so each `name` is an alias. The event is reported even when the batch names tools outside the round's scope.

A round that ends in tool calls reports no `assistant_reply`, but it still counts as a round, and its `model_turn_completed`, and its debug events when capture is on, still fire. Each `models.loop` round reports its round events and, for a tool round, the outcome of each tool call.

### Round numbers

`turn` numbers the rounds. Each round advances a 1-based [round count](11-conversations.md#turns-and-live-output), and a round's `thinking`, reply, degraded-metadata, and debug events all hold that round's number in `turn`. A spawned task keeps its own round count, so a task's first round is `turn` 1 whatever its owner has done, and a task's rounds report under that task, labeled with the section its chain is running. The round count stops at 4294967295 instead of wrapping, so a `turn` value is never reused.

The round count is separate from the round cap, which limits each `models.loop` call on its own, as [The round cap](11-conversations.md#the-round-cap) describes. The run's default limits are gathered in [Limits at a glance](17-limits-and-errors.md#limits-at-a-glance).

### Event order within a round

A round's events come in a fixed order:

1. `request` and `response`, only when the host has debug capture switched on
2. `model_turn_completed`, or `model_turn_failed` in its place
3. one `model_metadata_degraded` for each metadata problem
4. `thinking`, when the answer has non-empty reasoning
5. `model_turn_truncated`, when a text reply's finish reason is `length`
6. `assistant_reply` for a text reply, or `assistant_tool_calls` for a batch of tool calls

For example, a `models.infer` round whose answer includes reasoning reports:

````text
model_turn_completed
thinking
assistant_reply
````

### models.infer rounds

A `models.infer` round is a full round. It advances the round count and reports the same round events as a `models.loop` round, including the debug pair, `model_turn_completed`, `thinking`, and `model_turn_truncated` when the reply is cut off at `length`. Each `models.infer` call is exactly one completed round followed by one `assistant_reply` with `origin` `infer`, with no `thinking` between them when the answer holds no reasoning. It never reports tool call events, because a `models.infer` round has no tools in scope.

### Degraded metadata

Malformed or unusual metadata from the backend does not fail the round. Instead, the run reports one `model_metadata_degraded` event per problem, with `turn`, the number of the round that served the answer, and a `message` that names the malformed part and why it did not parse. The round still succeeds, and its reply still arrives. The backend's `usage`, `timings`, and `metrics` parts are checked independently, so each malformed one is dropped and reported once while the others are kept. The messages are:

- ``malformed `{key}` in completion response ignored: {reason}``, where `{key}` is `usage`, `timings`, or `metrics`, and `{reason}` is the decoder's own reason, as in ``malformed `usage` in completion response ignored: invalid type: string "lots", expected u64``
- ``completion response named no string `model`; recorded as empty``, when the answer names no model. It comes ahead of any messages about the other parts, and the round's `model` is then an empty string.

A healthy backend leaves a clean history: well-formed metadata, and parts the backend simply leaves out, report no `model_metadata_degraded`. `models.infer` rounds get these reports exactly as `models.loop` rounds do.

### Debug capture

When the host switches debug capture on, each round also reports its raw bodies, as sent to and received from the backend, under the section. `request` holds `turn` and the full, unredacted `body`, and `response` holds `turn`, `body`, and, when the backend supplied them, `finish_reason` and `reasoning_content`. The bodies are raw and include the full prompt. Capture is a host setting that a prompt cannot change: by default neither event is reported, and the standard runner leaves capture off.

## Model call metrics

The `metrics` field of an `assistant_reply` event holds everything measured about one model call. It is nil when nothing was measured, so test it with a plain truth test before reading inside it. Events are the only place a prompt can read these numbers, since `models.infer` returns only the reply text and `models.loop` returns nil. This prompt reports the tokens its two rounds used:

````markdown
---
name: token-report
description: Writes a short talk and reports the tokens it used
promptforge: 0
models:
  writer: {}
---

# Token report

```lua
models.default('writer')
```

## Talk

```lua
local outline = models.infer('Outline a two-minute talk about honeybees.')
local talk = models.infer('Write the talk from this outline: ' .. outline)
local prompt_tokens, completion_tokens = 0, 0
for _, e in ipairs(tasks.events(sys.taskid)) do
  local usage = e.kind == 'assistant_reply' and e.metrics and e.metrics.usage
  if usage then
    prompt_tokens = prompt_tokens + usage.prompt_tokens
    completion_tokens = completion_tokens + usage.completion_tokens
  end
end
return talk .. '\n\n' .. prompt_tokens .. ' prompt tokens, ' .. completion_tokens .. ' completion tokens'
```
````

The run result is the talk followed by the token totals of both rounds. The `usage` line guards each lookup, because `metrics` and each of its parts can be nil.

### Where the numbers come from

`metrics` has four optional parts. `usage`, `llama`, and `vllm` come from the serving backend, and `client` comes from the calling client's own clock. A part whose source did not report is nil. `llama` is present only when a llama.cpp server served the call and `vllm` only when vLLM did, so a present part tells you which backend served it:

````lua
local m = e.metrics
local backend = (m and m.llama and 'llama.cpp') or (m and m.vllm and 'vLLM') or 'unknown'
````

Counts are integers: the `*_tokens` fields, `prompt_n`, `predicted_n`, `draft_n`, and `draft_n_accepted`. Times and rates are floating-point numbers. `metrics` itself, each of its four parts, and each optional field can be nil, so guard every level of a read.

### Token usage

`metrics.usage` holds the call's token counts:

| Field | Value |
|---|---|
| `prompt_tokens` | Tokens in the prompt, always present when `usage` is |
| `completion_tokens` | Tokens the model generated, always present when `usage` is |
| `total_tokens` | `prompt_tokens` plus `completion_tokens`, always present when `usage` is |
| `cached_tokens` | Prompt tokens served from the backend's prefix cache, or nil when the backend does not report it |
| `reasoning_tokens` | Tokens spent on reasoning, or nil when the backend does not report it |

### llama.cpp timings

`metrics.llama` is present only when a llama.cpp server served the call, and then all eight of its fields are numbers:

| Field | Value |
|---|---|
| `prompt_n` | Prompt tokens processed |
| `prompt_ms` | Wall-clock milliseconds spent processing the prompt |
| `prompt_per_second` | Prompt tokens processed per second |
| `predicted_n` | Tokens predicted |
| `predicted_ms` | Wall-clock milliseconds spent predicting |
| `predicted_per_second` | Tokens predicted per second |
| `draft_n` | Draft tokens proposed by speculative decoding |
| `draft_n_accepted` | Draft tokens the target model accepted |

Compare the last two to measure speculative decoding. The acceptance ratio is `draft_n_accepted / draft_n` when `draft_n > 0`:

````lua
local llama = e.metrics and e.metrics.llama
local acceptance
if llama and llama.draft_n > 0 then
  acceptance = llama.draft_n_accepted / llama.draft_n
end
````

### vLLM metrics

`metrics.vllm` is present only when vLLM served the call. Each of its fields is nil when vLLM did not measure it:

| Field | Value |
|---|---|
| `time_to_first_token_ms` | Milliseconds until the first token |
| `generation_time_ms` | Milliseconds spent generating |
| `queue_time_ms` | Milliseconds spent waiting in the scheduler queue |
| `mean_itl_ms` | Mean inter-token latency, in milliseconds |
| `tokens_per_second` | Tokens per second |

### Client timing

`metrics.client` is measured on the calling client's own clock:

| Field | Value |
|---|---|
| `e2e_ms` | End-to-end milliseconds, from sending the request until the whole answer arrived, always present when `client` is |
| `ttft_ms` | Milliseconds from sending the request to the first streamed token, or nil when the stream produced none |
| `mean_itl_ms` | Mean inter-token latency, in milliseconds, or nil unless at least two tokens streamed |

The client's `mean_itl_ms` comes from the client's own clock, so it is a separate measurement from `vllm.mean_itl_ms`.

## Tool call events

Every tool call reports `tool_call_succeeded` or `tool_call_failed`, whether it is a [script call](12-tools.md#calling-tools-from-lua) made with `tools.call`, a [model tool call](12-tools.md#model-tool-calls) made inside `models.loop`, or a call to one of the model's [task built-ins](15-tasks.md#letting-the-model-start-tasks). The outcome is filed under the section and task that made the call, for script and model calls alike, and a failed model tool call is reported before it becomes the failure text the model reads. A call's output is then reported as a `tool_result` event. This prompt gives the model a local tool and lists the calls the model made:

````markdown
---
name: tool-trace
description: Lists the tool calls the model made
promptforge: 0
models:
  writer: {}
---

# Tool trace

## Add

```lua
models.use('writer')
tools.add_local('add', 'Add two integers', { a = 'integer', b = 'integer' }, function(p)
  return p.a + p.b
end)
local msgs = messages.new():user('Use the add tool to add 17 and 25, then state the sum.')
models.loop(msgs)
local trace = {}
for _, e in ipairs(tasks.events(sys.taskid)) do
  if e.kind == 'assistant_tool_calls' then
    for _, c in ipairs(e.calls) do
      trace[#trace + 1] = 'round ' .. e.turn .. ' asked for ' .. c.name .. ' as ' .. c.id
    end
  elseif e.kind == 'tool_result' then
    trace[#trace + 1] = 'round ' .. e.turn .. ' got ' .. e.content .. ' from ' .. e.alias .. ' as ' .. e.tool_call_id
  end
end
return table.concat(trace, '\n')
```
````

The block returns lines such as:

````text
round 1 asked for add as call_1
round 1 got 42 from add as call_1
````

The model's first round asks for `add`, and that batch is reported as `assistant_tool_calls` before the handler runs. The handler's return value then comes back as a `tool_result` with the same round number and call id. The model's final text reply adds nothing to the trace.

### Tool results

A `tool_result` event holds these fields beside the coordinates:

| Field | Value |
|---|---|
| `turn` | For a model tool call, the turn of the round that requested it; for a script call, the round count when the call was made |
| `tool_call_id` | The provider-issued call id for a model tool call, or the empty string for a script call |
| `alias` | The alias the call named |
| `content` | The tool's output, already wrapped in the [untrusted envelope](09-the-store.md#wrapping-untrusted-text) when the tool is untrusted |
| `trusted` | `true` only when the output was not wrapped |

Whether a call reports a `tool_result` depends on who made it and how it ended:

| Call | `tool_result` |
|---|---|
| A script call that succeeds | Yes, after `tool_call_succeeded` |
| A script call that fails | No |
| A script call refused with kind [`unbound_tool`](12-tools.md#calling-tools-from-lua) because its alias names no bound tool, a task built-in's name included | No |
| A model tool call to a bound tool that succeeds | Yes, after `tool_call_succeeded` |
| A model tool call to a bound tool that fails | Yes, after `tool_call_failed`, with the wrapped failure message as `content` |
| A local tool call whose handler returns | Yes, with `trusted` set to `true` |
| A local tool call whose handler raises or returns a table | No |
| A task built-in call by the model, served or refused | Yes, with the built-in's name as `alias` |

`tool_call_id` tells the two kinds of call apart: a model tool call has the model's id, and a script call's id is empty. A script call's `turn` is the number of rounds already completed when the call was made, and its `content` is exactly what the script received, wrapped when the tool is [untrusted](12-tools.md#trusted-and-untrusted-output).

### Matching results to requests

A model tool call's `tool_result` fires exactly once, with the round's `turn`, the model's call id, the alias, the final content, and the trust flag, so you can match it to the call in `assistant_tool_calls` that asked for it. Match on `turn` together with the id, because providers reuse ids such as `call_1` across rounds. Within one message list the ids are unique, as [Checking the list](11-conversations.md#checking-the-list) requires, and pairing `turn` with the id tells calls apart across a task's whole history.

In a history, a tool round reads in order: the batch of requested calls, with the model's name and the call names, comes before any call runs, and then each result is reported under the model's call id and the tool's alias. The model calls tools by alias, so `name` in `assistant_tool_calls` and `alias` in `tool_result` are both prompt-local names.

### Scope checks

Before each model round, the round's [scope](12-tools.md#advertising-tools-to-the-model) is checked. `tool_scope_validation_started` is reported before the scope is built from the round's bound tools, local tools, and task built-ins, and then `tool_scope_validation_succeeded` or `tool_scope_validation_failed` follows, all under the section. A failed check's error goes back to the code that asked for the round. A model tool call that names a tool outside the round's scope is refused and reports `tool_call_failed`.

### Local tool calls

Each call to a [local tool](12-tools.md#local-tools) reports `tool_call_succeeded` or `tool_call_failed` for the section that ran it. A handler that returns reports a trusted `tool_result` whose `turn` is the round count recorded when the call was made, even when the handler runs rounds of its own. A handler that raises, or that returns a table, reports `tool_call_failed` before the error reaches the caller, and no `tool_result`.

The handler runs inside the calling chain, so everything it does is ordinary work whose events land in the calling task's history: its store operations, its model rounds, and its own tool calls, which are script calls. All of those events come before the outer call's `tool_call_succeeded`. When a handler calls a second local tool, the inner call finishes first, so nested local calls report their `tool_result` events innermost first.

### Task built-in calls

Each task built-in call the model makes reports `tool_call_succeeded` when it is served, or `tool_call_failed` when it is refused, under the section that ran the loop. It also reports a `tool_result` whose `alias` is the built-in's name and whose `content` is the text the built-in sent back, with its `turn`, the model's call id, and its trust flag.

The model's history read shows up as a `tool_result` with `alias` `task_events` under the model's call id. When the read found events, that result is untrusted, with `trusted` set to `false`. An empty read answers with the trusted sentence `no new events`.

## Task lifecycle events

Every task start reports a `task_started` event under the section that started the task, whether the task came from `tasks.spawn`, a `fanout` arm, or the model's `task` built-in. This prompt lists the tasks a fanout started:

````markdown
---
name: arm-starts
description: Lists the tasks a fanout started
promptforge: 0
---

# Arm starts

## Gather

```lua
fanout('### Worker', { 'a', 'b', 'c' })
local starts = {}
for _, e in ipairs(tasks.events(sys.taskid)) do
  if e.kind == 'task_started' then
    starts[#starts + 1] = e.task .. ' ' .. e.origin .. ' ' .. e.target
  end
end
return table.concat(starts, '\n')
```

### Worker

```lua
return item
```
````

````text
0.0 author Worker
0.1 author Worker
0.2 author Worker
````

Each [arm](14-fanout.md#inside-an-arm) of the fanout is a task, so the fanout reports one `task_started` per arm into the caller's own history, under the calling section, `Gather`. Everything that happens inside an arm, from its section events and store results to its `models.infer` rounds, lands in that arm's own history under the arm's id, and each arm ends with `task_succeeded` under the worker section, `Worker`.

### Task starts

`task_started` holds these fields beside the coordinates:

| Field | Value |
|---|---|
| `task` | The new task's dotted id |
| `target` | The heading text of the section where the task's chain starts, without the `#` markers |
| `origin` | `"author"` or `"model"` |
| `input`, `item`, `index` | The seeds the task started with, each nil when unset, as [Starting a task](15-tasks.md#starting-a-task) describes |
| `var` | The snapshot of the owner's `var` that the task started with |

`task_started` is stamped on the owner's `seq` count, not the new task's. The new task's own events hold its id in `provenance.task`, so `task_started` events and provenance together are enough to rebuild the tree of tasks. `origin` says who started the task: `author` for a task from `tasks.spawn` or a `fanout` arm, and `model` for a task the model started with its `task` built-in. A model-started task's `task_started` sits under the owner's section, with `origin` `model` and a `target` naming the section without the `##`.

### How a task ends

Every started task gets exactly one terminal event:

| Kind | Reported when the task |
|---|---|
| `task_succeeded` | ended with a result |
| `task_failed` | ended with an error |
| `task_cancelled` | was cancelled |
| `task_abandoned` | was ended along with its owner, with the reason in `reason` |

The terminal event is reported under the task's target section, stamped with the task's own provenance, and holds the task's id in `task`. A task is never reported as both abandoned and cancelled. `tasks.events(t)` on a finished task returns its events in order, ending with its terminal event.

A model-started task's `task_failed` is reported under its target section, not the owner's, and the owner reads it with `tasks.events` on the model task's id. An arm whose `models.loop` ran into its round cap reports `task_failed`, even though `fanout` still gives that arm an exhausted result and cancels no sibling, as [Arm failures](14-fanout.md#arm-failures) describes. The arms' `task_started` events sit in the caller's own history, and each arm's terminal event is in that arm's history, which the caller owns and may read.

A [`tasks.cancel`](15-tasks.md#cancellation-and-task-lifetimes) reports one `task_cancelled` for the task, under its target section and stamped with the task's own provenance, after the events of everything the cancelled task owned. Cancelling it again reports nothing.

### Abandoned tasks

`task_abandoned` says why a live task was ended along with its owner, in its `reason` field:

| `reason` | When | Notice phrase |
|---|---|---|
| `owner_returned` | The owner returned while the task was live | `the section ended` |
| `owner_failed` | The owner failed while the task was live | `the owner failed` |
| `tool_loop_exhausted` | The owner's `models.loop` ran past its round cap | `the tool loop was exhausted` |
| `owner_aborted` | The owner was itself aborted, as happens to the tasks nested under an abandoned task | `the owner was aborted` |
| `run_terminated` | The run was cancelled or ended by the host | `the run ended` |

An abandonment is distinct from a purposeful cancel. For an author task, `owner_returned` goes with the [`tasks_live`](15-tasks.md#cancellation-and-task-lifetimes) error on the owner, while for a model task it is a quiet abandon. In a nested abandonment, everything the abandoned task owned reports its end first, and the task's own `task_abandoned` comes last. Before `run_succeeded` or `run_failed`, the run settles every task still live by abandoning it exactly once: `run_terminated` for a task still live at the end, and `owner_aborted` for the tasks nested under it.

A prompt never reads `task_abandoned` through `tasks.events`. It is stamped on the abandoned task after the task's owner has already ended, so it appears only in the host's run log.

### Task notices

A [task notice](15-tasks.md#task-notices-to-the-model) is the sentence queued for the model when a task it started ends. Every notice reports a `task_notice` event under the owner's section the moment it is queued, even if no round ever reads it. Its fields are `turn`, the owner's round count when the notice was queued, `task`, the ended task's id, and `text`, the exact sentence the model reads, byte for byte. Notices exist only for tasks the model started, and the text takes one of four forms:

````text
Task id={id} (## {target}) completed: {result}
Task id={id} (## {target}) failed: {error}
Task id={id} (## {target}) was canceled: the author cancelled it
Task id={id} (## {target}) was abandoned: {phrase}
````

In a completed notice, `{result}` arrives in the untrusted envelope. The cancel sentence spells `canceled` with one l and `cancelled` with two, so match it exactly as shown. In an abandoned notice, `{phrase}` is the notice phrase for the `reason`, from the table above, and those phrases appear only in notice text, never in `task_abandoned`. The model's own `task_cancel` queues no notice, because the model already read the built-in's confirmation. [`tasks.note`](15-tasks.md#checking-on-tasks) reports no event at all, so a note never shows up in any history.

## Trust and what events leave out

Events say that something happened, not everything about it. No `_failed` event holds a message: each says only that something failed, and the detail is the failing call's own error. A failed store call, for example, raises an error value of kind `lua` that `pcall` catches, while its `store_*_failed` event holds none of that detail. In the same way, a run's outcome and its error detail come from the run's result and from the errors raised to Lua, never from events.

### Untrusted text in events

Treat the text inside events as untrusted:

- `thinking.text` and `assistant_reply.text`
- `assistant_tool_calls.calls`, names and arguments alike
- `tool_result.content`, unless `trusted` is `true`
- `user_input.text`
- the seeds in `task_started`
- the `request` and `response` bodies
- `model_metadata_degraded.message`

The coordinates come from the host, for `execution`, and from your own headings, for `section`. Before you hand event text to a model, wrap it with [`untrusted()`](09-the-store.md#wrapping-untrusted-text) as you would any other untrusted text:

````lua
local t = tasks.spawn('### Research')
tasks.when_any({ t })
local found = {}
for _, e in ipairs(tasks.events(t)) do
  if e.kind == 'assistant_reply' then
    found[#found + 1] = untrusted(e.text)
  end
end
return models.infer('Summarize what the research found:\n' .. table.concat(found, '\n'))
````

A `tool_result` whose `trusted` is `false` already arrives in the untrusted envelope.

### Events a prompt never reads

A prompt reads the events of its own task and of the tasks it owns, as reported so far. No prompt ever reads these, which appear only in the host's run log:

- `run_succeeded` and `run_failed`, which are reported after every chain has stopped
- every `task_abandoned`, which is stamped on the abandoned task after its owner has already ended
- `parse_failed` and `lua_compilation_failed`, because a file whose parse fails never runs

Two more limits depend on who is reading. A task never reads its own terminal event, because it has ended by then, so only its owner reads it. A reader never sees the events of a task it neither owns nor runs inside, such as a task started by one of its own tasks, although that task's start still shows up as `task_started` in the history of the task that started it.

### Events never steer the run

The engine acts on no event. The only way an event comes back is an explicit history read, a prompt's `tasks.events` or the model's `task_events`, which the host serves from its log, so a host that drops events changes what those reads return. Each model reply and each batch of tool calls appears whole, once its round completes: the partial fragments a host may stream live never become events.
