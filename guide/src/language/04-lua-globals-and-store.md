# Lua Globals and the Store

Every section runs sandboxed Lua, but it does not run empty-handed. This chapter teaches the globals the runtime seeds into each section, `args` and `argv`, `sys`, `var`, `prose`, and `log`, plus the run-scoped `store` where a prompt keeps its bulk state. These are your everyday tools, so we take them one at a time.

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

Every section receives a `sys` JSON value carrying `when`, `id`, `section_name`, `execution`, and `section_count`.

The `sys.when` value is the run's start time as a UTC RFC 3339 string. The host stamps it once when the run begins, so every section agrees on when the run began, and two runs given the same start time read the same value.

The `sys.id` value is a hierarchical id rendered as a dot-separated path: the running chain's id followed by the entry's position in that chain. The main walk is chain `0`, so the H1 pass is `0.0` and the walked sections are `0.1`, `0.2`, and so on; a `call` child or a fanout arm is a child chain of its caller (`0.0`, `0.1`, ...) whose entries nest under it (`0.0.0`, `0.0.1`, ...). Every entry's id is unique within a run, so entering the same section twice yields two distinct ids, and two runs of the same prompt with the same inputs yield the same ids.

One field is conditional. `sys.index` exists only when the section runs as one arm of a fanout, a concurrent walk over a collection. Reading it in an ordinary walked section raises an unknown-field error. Arms of a nested fanout restart `sys.index` numbering at 1.

Once the section has dispatched its first model or tool call, `sys.model` reads the catalog id of the model the section resolved. Reading it before that first dispatch raises an unknown-field error.

## log: checkpoints

Call `log(...)` from any section's Lua block to emit a checkpoint. Checkpoints are reported through the run's observer under the current section name, which makes them the simplest way to trace a run.

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

## Designed, not yet built: the prompt global

A `prompt` reflection global is designed but not yet built. It will expose the prompt's own declaration to section Lua - the declared model roles, tool slots, and args - so a prompt can adapt its behavior to how it was satisfied. Today the declaration is visible to the host that runs the prompt, not to the prompt's own code.

