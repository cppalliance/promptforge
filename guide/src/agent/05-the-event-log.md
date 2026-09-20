# The event log

This chapter teaches you how your agent reads what has already happened in the run. The harness keeps an append-only log of every event the run reports, and `tasks.events` gives your program a window into it. Your context building reads this log, so learn the read rules exactly.

## Read the log

````lua
local events = tasks.events(sys.taskid)
for i = 1, #events do
  local event = events[i]
  log(event.kind)
end
````

`tasks.events(task, opts?)` returns a plain sequence of the events the named task has reported so far, in the task's own order. `sys.taskid` names the task your section runs inside, so an agent reading its own history passes it. An agent that started background work with `tasks.spawn` reads a child's history by passing the child's handle instead; a task may read itself or a task it owns, and nothing else.

Each entry is an ordinary Lua table. `#events` gives the number of entries returned, `events[1]` is the earliest, and the table is yours: index it, filter it, or hand entries to a function that changes them. The mutation cannot reach the log.

## Read incrementally

````lua
local last = var.last_seen
local fresh = tasks.events(sys.taskid, { last = last })
for _, event in ipairs(fresh) do
  var.last_seen = event.provenance.seq
end
````

Every event carries `provenance`, a table with `task` and `seq`. The `seq` value is the event's position within its task, and it only ever grows. Pass the highest `seq` you have already seen as `opts.last` and the call returns only later events, so a loop that runs once per turn reads each event exactly once. Store the cursor in `var` and it survives across sections.

## Reads stay deterministic

The log grows only when the run resumes from a host call, never in the middle of a chunk. The read is itself a host call: what `tasks.events` returns is fixed at the moment it returns, and no entry appears, moves, or changes inside the table afterwards. Two runs given the same inputs and the same answers see the same events in the same order.

## What an entry carries

Every entry carries `kind`, `execution`, `section`, and `provenance`. The `kind` reads as a pinned snake_case label, such as `assistant_reply`, `tool_result`, `user_input`, `lua`, or `task_started`, and the rest of the table is that kind's own fields.

A model round leaves `assistant_reply` (with `turn`, `text`, `model`, `finish_reason`, and `metrics`) or `assistant_tool_calls` (with the requested `calls`); a block of reasoning leaves `thinking`. Every dispatched tool call leaves `tool_call_succeeded` or `tool_call_failed`, and a call the model issued also leaves `tool_result` carrying `turn`, `tool_call_id`, `alias`, `content`, and `trusted`. Operator text arrives as `user_input`, and your own `log(...)` checkpoints as `lua` with `message`. Background work leaves `task_started` with its spawn seeds, then one of `task_succeeded`, `task_failed`, `task_cancelled`, or `task_abandoned`.

An absent optional field, such as a reply with no `finish_reason`, reads as nil, so test presence with a plain truth test.

## History across runs

A relaunched agent runs under a new task record, but the session's transcript persists: the harness writes every event to its run log before the next effect is issued, and a client reading the transcript sees every run the session has made, in order. Your program's own view through `tasks.events` covers the current run.

The `tasks` namespace is part of the prompt language, so the same call works in an unattached prompt. The session extras, `user_input()` and `ui()`, are what mark the agent environment.
