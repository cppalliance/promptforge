# The event log

This chapter teaches you how your agent reads what has already happened in the run. The host keeps an event log, and `runtime.events()` gives your program a window into it. Your context building reads this log, so learn the read rules exactly.

## Read the log

````lua
local events = runtime.events()
for i = 1, #events do
  local event = events[i]
  log(event.kind)
end
````

`runtime.events()` returns a read-only indexed view over the host's event log. `#events` gives the number of visible events. Read one event at a time by position: `events[1]` is the first visible event. Each positional read brings only that single entry into Lua, so indexing a long history never bulk-copies the log.

## Reads stay deterministic

The view grows only at host-call resumes, never mid-chunk. Between two host calls, `#events` does not change and no entry appears or moves. Reads you make between suspensions stay deterministic, so you can loop over the view without guarding against growth.

## Index safely

````lua
local second = events[2.0]
local a = events[0]
local b = events[-1]
local c = events.latest
````

Indexing follows ordinary Lua rules and never fails. A float key with an exact integer works like ordinary indexing: `events[2.0]` reads entry 2. An out-of-range index reads nil. Zero, negative, and non-numeric keys read nil, so `events[0]`, `events[-1]`, and `events.latest` never error. Even an in-bound entry the log no longer holds reads nil. Reads never fail your program.

## History is read-only

The view is read-only. Assigning into it, as in `events[1] = 'x'`, raises an error. Your program cannot rewrite history.

A fetched entry is a fresh table. Mutate it freely: add fields, reorder them, hand the table to a function that changes it. The mutation cannot reach the log.

## What an entry carries

Each entry carries fields such as `kind` and `content`. The `kind` reads as a pinned label, such as "agent_message", and `content` holds the entry's text. Entries also carry metadata you use to reconstruct context: `section`, `chain_id`, `depth`, `turn`, `model`, `tool_call_id`, `finish_reason`, and `metrics`.

Tool activity leaves a clear trail. Every dispatched tool call emits a tool-call-succeeded or tool-call-failed event. Each `tool_call` also emits a tool-result event that carries the chain id, the execute depth, the completed model-turn count, the tool alias, the final content, and the trust flag.

## History across runs

A relaunched agent sees its whole persisted history from its first instruction. The view starts with everything the log already holds.

Run the same code with no log configured and `runtime.events()` returns a plain empty table of length 0. The read loop still works; it just iterates zero times.

The `runtime` global exists only in an agent. Its presence proves the agent environment.

