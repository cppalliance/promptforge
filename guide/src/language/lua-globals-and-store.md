# Lua Globals and the Store

Every section runs sandboxed Lua, but it does not run empty-handed. This chapter teaches the globals the runtime seeds into each section, `args`, `sys`, `var`, `reply`, and `log`, plus the run-scoped `store` where a prompt keeps its bulk state. These are your everyday tools, so we take them one at a time.

## args: the run's input

Every section's Lua block can read the run's argument string through the `args` global:

````lua
log('the run was started with: ' .. args)
````

## sys: runtime metadata

Every section receives a `sys` JSON value carrying `when`, `now`, `id`, `section_name`, `execution`, and `section_count`.

The `sys.when` and `sys.now` values are the current UTC time formatted as RFC 3339 strings. The `when` value is stamped once at the walk's start, so every section agrees on when the run began, while `now` is fresh at each read.

The `sys.id` value is a run-global counter. The H1 pass keeps id 0, and every section entry and every fanout arm takes the next value, so entering the same section twice yields two distinct ids.

One field is conditional. `sys.index` exists only when the section runs as one arm of a fanout, a concurrent walk over a collection. Reading it in an ordinary walked section raises an unknown-field error. Arms of a nested fanout restart `sys.index` numbering at 1.

After a section's prose block finishes, the model's finish reason is recorded into `sys.reply_finish_reason`, so the prompt can inspect why generation stopped.

## log: checkpoints

Call `log(...)` from any section's Lua block to emit a checkpoint. Checkpoints are reported through the run's observer under the current section name, which makes them the simplest way to trace a run.

## var: the per-run clipboard

The `var` table is a per-run clipboard. It is seeded into each section's Lua state on entry and read back before teardown, so the next section sees the updates:

````lua
var.topic = 'governance'
````

Two rules keep the clipboard safe. Reassigning the `var` global itself fails the run; you mutate its fields, never replace it. And assigning a non-JSON value to a field fails, naming the field and the type: `var.f = function() end` errors because a function is not JSON data.

## reply: the rolling result

Each section entry is seeded with the previous section's final reply, and the section's own final reply replaces it for the next. You can assign `reply` directly, but the value must be nil or a string; anything else fails with a Lua error.

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

