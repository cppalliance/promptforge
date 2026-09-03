# Files and variables

This chapter teaches you how your agent works with files and per-run state. The `store` table reaches files, the `var` global holds run state, and `log` writes to the event stream. These are the calls that let your agent leave something behind, so their limits are worth learning.

## Write and change files

````lua
store.write('notes.txt', 'first line\n')
store.append('notes.txt', 'second line\n')
store.str_replace('notes.txt', 'first', 'opening')
store.delete('draft.txt')
````

The `store` table is always installed. Tool scoping never removes it. Data you write through `store` persists past the run, and the host sees it after the run.

`store.write(path, contents)` writes a file and returns nil. `store.append(path, contents)` appends text to a file and returns nil. `store.str_replace(path, old, new)` replaces text in a file and returns nil. `store.delete(path)` deletes a file and returns nil.

## Read files

````lua
local whole = store.read('notes.txt')
local tail = store.read('notes.txt', 10)
local slice = store.read('notes.txt', 10, 20)
local numbered = store.read_numbered('notes.txt', 10, 20)
````

`store.read(path)` returns the file's contents verbatim. Add a 1-based `start` line to read from that line to the end of the file. Add an `end` line to read an inclusive range. Passing `end` without `start` is an error: "start is required when end is given". `store.read_numbered(path, start, end)` returns the file with absolute line numbers and takes the same optional bounds.

## Find files

````lua
local paths = store.glob('*.txt')
if store.exists('notes.txt') then
  log('notes exist')
end
````

`store.glob(pattern)` returns an array table of matching paths. `store.exists(path)` reports whether a path exists.

## When a store call fails

A failed store operation aborts the running chunk with an error. Every store operation, success or failure, records an event in the run's event stream, so the log shows what your agent touched.

## Hold run state in var

````lua
var.count = 1
var.seen = { 'alpha', 'beta' }
var.note = 'ready'
log(var.note)
if var.missing == nil then
  log('nothing stored yet')
end
````

The `var` global stores per-run state. Absent keys read as nil, which is why the `var.missing` check passes. Your program also reads host-supplied run variables through `var`.

Assign only JSON-representable values into `var`. Functions, userdata, and threads are rejected at the assigning line. Nested tables you assign come back guarded, so later writes into them cross the same validation. When a write fails, the error names the dotted path of the offending field, such as `var.a.b`. Reassigning the `var` global itself is detected and reported; write `var.<field>` instead.

## Record messages with log

`log(message)` records a message in the run's event stream, where later context building can read it. The call takes exactly one argument, and it must be a UTF-8 string. Keep the message to at most 256 characters, with no newline or control characters.

Log calls are capped by a per-run event budget and a cumulative byte budget. The event budget is configured per run with `lua_log_events`, and the default is 1024 events. An exhausted budget fails the call.

## Ask the operator

````lua
tool_call('user_input', {})
````

Request the operator's next message by invoking the `user_input` tool through `tool_call`. The operator's answer arrives in the event log as a `user_message` event, where your context building can read it.

