# Lua Globals and the Store

Every section runs sandboxed Lua, but it does not run empty-handed. This chapter teaches the globals the runtime seeds into each section, `args` and `argv`, `sys`, `var`, `prose`, and `log`, the run-scoped `store` where a prompt keeps its bulk state, and the `tasks` namespace for background work. These are your everyday tools, so we take them one at a time.

## args and argv: the run's input

Every section's Lua block can read the run's exact argument string through the `args` global:

````lua
log('the run was started with: ' .. args)
````

The `argv` global is the parsed form of that string, shaped by the prompt's `args:` declaration. A prompt with no `args:` key has the default declaration - one optional string field named `prose` - and the interface wraps the argument string into it, so `argv.prose` reads the input text on every channel, the empty string included. A prompt with a structured `args:` declaration receives its argument string as JSON: the call argument `{"query": "papers", "limit": 5}` arrives as a table with `argv.query` and `argv.limit`. When the string does not parse as JSON, or parses as `null`, `argv` is nil, so `if argv then` is the idiomatic malformed-input check.

Optional means absent. A call that omits an optional field leaves `argv.field` nil; absent is not the empty string, and a present empty string is a real value the caller chose to send.

### The H1 repair pattern

The `argv` global is writable in the H1 pass and frozen everywhere else. A prompt that tolerates malformed input reads the raw `args`, computes the repair, and assigns it:

````lua
if not argv then
  argv = { query = args }
end
````

The executor reads the value back when the H1 pass completes, and every later section sees the repaired value frozen: reads work, absent fields read nil, and any assignment - `argv = ...` or a field write at any depth - fails with an error naming the freeze.

## sys: runtime metadata

Every section receives a `sys` JSON value carrying `when`, `id`, `taskid`, `section_name`, `execution`, and `section_count`.

The `sys.when` value is the run's start time as a UTC RFC 3339 string. The host stamps it once when the run begins, so every section agrees on when the run began, and two runs given the same start time read the same value.

The `sys.id` value is a hierarchical id rendered as a dot-separated path: the running chain's id followed by the entry's position in that chain. The main walk is chain `0`, so the H1 pass is `0.0` and the walked sections are `0.1`, `0.2`, and so on; a `call` child, a fanout arm, or a spawned task is a child chain of its caller (`0.0`, `0.1`, ...) whose entries nest under it (`0.0.0`, `0.0.1`, ...). Every entry's id is unique within a run, so entering the same section twice yields two distinct ids, and two runs of the same prompt with the same inputs yield the same ids.

The `sys.taskid` value is the id of the task the section runs inside: the nearest enclosing task, which is the main walk's `0` for an ordinary walked section, the arm's own task inside a fanout, and the spawned task inside a `tasks.spawn` chain. A `call` child reports its caller's task, since a `call` blocks its caller and the two never interleave. It is the handle a section passes to `tasks.status` or `tasks.events` to read its own record.

One field is conditional. `sys.index` exists only when the section runs as one arm of a fanout, a concurrent walk over a collection. Reading it in an ordinary walked section raises an unknown-field error. Arms of a nested fanout restart `sys.index` numbering at 1.

Once the section has dispatched its first model or tool call, `sys.model` reads the catalog id of the model the section resolved. Reading it before that first dispatch raises an unknown-field error.

## log: checkpoints

Call `log(...)` from any section's Lua block to emit a checkpoint. Checkpoints are reported as events under the current section name, which makes them the simplest way to trace a run.

## var: the per-run clipboard

The `var` table is a per-run clipboard. It is seeded into each section's Lua state on entry and read back before teardown, so the next section sees the updates:

````lua
var.topic = 'governance'
````

Two rules keep the clipboard safe. Reassigning the `var` global itself fails the run; you mutate its fields, never replace it. And assigning a non-JSON value to a field fails, naming the field and the type: `var.f = function() end` errors because a function is not JSON data.

## prose: the pending Markdown

The prose written since the section's heading or last Lua block is available to the next Lua block as the `prose` global. It is lazy: the `{{ }}` placeholders in it are substituted on the first read, not at block entry, so a block that never reads `prose` never evaluates it. The value is read-only and memoized - assigning to it fails, and every read after the first returns the same substituted text. Each prose buffer is fresh: a second prose block in the same section evaluates independently for the Lua block that follows it.

````lua
local answer = models.infer(prose)
````

## store: virtual files

The run-scoped `store` persists bulk state as virtual files addressed by logical string paths, shared across every section of the run. The core operations read and write whole files:

````lua
store.write('state.txt', 'first')
store.append('state.txt', '\nsecond')
local text = store.read('state.txt')
if store.exists('state.txt') then
  log('state is present')
end
````

The call `store.write(path, text)` writes a virtual file, `store.append(path, text)` appends to it, `store.read(path)` returns its verbatim contents, and `store.exists(path)` returns true when a store file is present.

Three more operations help with larger files. The call `store.read_numbered(path)` reads a file with absolute 1-based line numbers attached. Both `store.read` and `store.read_numbered` accept optional 1-based start and end line numbers that select a range, so `store.read_numbered('a.txt', 84, 85)` returns only lines 84 to 85, numbered. And `store.glob(pattern)` lists store entries matching a wildcard, as in `store.glob("ready-*.md")`.

## untrusted: guarding re-injected content

When store content goes back to the model, wrap it first. The `untrusted(text)` global wraps store content in a guard envelope before it is re-injected, so the model treats it as data rather than instructions.

## tasks: background work

The `tasks` namespace starts a section running in the background and lets the caller wait on it, inspect it, or end it. A `fanout` is built on the same machinery; `tasks` is the general form for work that does not fit one collection and one worker.

````lua
local t = tasks.spawn("### Research", { input = "governance", item = topic, index = 3 })
local task, ok, result = tasks.when_any({ t }, { timeout = 120 })
````

The call `tasks.spawn(target, opts?)` starts a chain at the named section and returns at once with a Task handle, a plain table `{ task = id }` with no methods. Every `tasks` function accepts the handle or the bare id string, so a handle stored in `var` survives intact. The options seed the chain: `opts.input` overrides its `args`, `opts.item` becomes its `item` global, and `opts.index` its `sys.index`; the caller's `var` is snapshotted into the chain, exactly as a fanout arm is seeded. The spawn shares `call`'s target resolution and depth cap, so the target must be in the caller's visible set.

The task runs until its section returns, fails, or is cancelled, and its result is delivered to the caller through one of the waits. A task the caller never waits on and never cancels is still live when the caller's section ends, and that is an error: the run fails with `tasks_live`. Every spawned task must be delivered or cancelled before its owner returns.

### The waits

The call `tasks.when_any(set, opts?)` parks the caller until the first member of `set` ends, or returns at once when one already has, and returns three values: the Task that ended, whether it succeeded, and its final text or error value. The error value is returned, never raised, so the caller decides. With `opts.timeout` in seconds, a wait that outlasts the timeout returns nil and the members keep running.

The call `tasks.when_all(set, opts?)` waits for every member and returns a results sequence with one `{ task, ok, result }` entry per member in input order, and a second value that is true when the timeout fired first. A failed member fills its entry with `ok = false` and the error value; it never raises, so no caller is forced into a cancel-or-leak choice for the members still running. When the timeout fires, the unfinished members' entries are absent.

A wait on a task the caller does not own raises `task_not_owned`, and a wait on a task whose result was already delivered raises `task_consumed`: each result is delivered exactly once.

### Inspection

The call `tasks.ready(task)` returns whether the task has ended, without waiting. The call `tasks.status(task)` returns a table with `target`, `origin` (`author` or `model`), `state`, `turns`, `depth`, the task's own live `tasks`, and, when present, `ok`, `section`, `blocked`, and `note`. The call `tasks.pending(filter?)` returns the caller's live tasks in spawn order, narrowed to `filter.origin` when given.

The call `tasks.events(task, opts?)` returns the events the task has reported so far, in the task's own sequence order. Each entry is a plain table in the event's serialized shape: `kind` (such as `assistant_reply`, `tool_result`, `lua`, or `task_started`), `section`, `provenance` with `task` and `seq`, and the kind's own fields. Pass `opts.last`, the highest `provenance.seq` already seen, to receive only later events, so a poll loop reads each event once. A section may read a task it owns or the task it runs inside, `sys.taskid`.

### Notes and cancellation

The call `tasks.note(text)` publishes the caller's own latest progress note, which its owner reads as `note` in `tasks.status`. The call `tasks.cancel(task)` ends a task the caller owns; cancelling a task that already ended does nothing.

## Designed, not yet built: the prompt global

A `prompt` reflection global is designed but not yet built. It will expose the prompt's own declaration to section Lua - the declared model roles, tool slots, and args - so a prompt can adapt its behavior to how it was satisfied. Today the declaration is visible to the host that runs the prompt, not to the prompt's own code.

