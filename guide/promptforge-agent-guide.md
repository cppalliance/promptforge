# Agent Programs

---

# Agent programs

This chapter teaches you what an agent is, the file you write, and how the Workshop runs it. Learn it first, because an agent is an ordinary PromptForge prompt document: everything the prompt language gives a prompt - sections, Lua blocks, model rounds, tools, the store - an agent has too. What makes it an agent is only where the file lives and who is listening.

## Write the smallest working agent

````markdown
---
name: hello
description: The smallest working agent.
promptforge: 0
---

# Hello

## Speak

```lua
log('hello from my agent')
```
````

Save that file as `hello.md` in the agents directory. The file is the whole agent: frontmatter that makes it a prompt, one title, one section, one Lua block. There is no manifest, no registration step, and no second file. When the host runs it, the `log` call records the message `hello from my agent` in the run's event stream, and the prompt runs to its end.

## How the Workshop runs an agent

The Workshop discovers agents by reading the agents directory: every `.md` file there is a launchable agent, listed under its file-stem name in a sorted list. Discovery reads the directory per request, so a file you add shows up in the agent list on the next connect, with no restart. A missing or unreadable directory is a state, not an error: the list simply offers the built-in chat alone.

Launching an agent parses the file as a PromptForge prompt and runs it on the unified document runtime, the same runtime that runs every other prompt. One launch is one prompt run: sections walk in order, Lua blocks suspend on host calls and resume with their answers, and the run ends when the document ends - or when the operator cancels it.

The directory itself is a configuration value: `agents.path` in `workshop.toml`. The default is `agents/` beside the config file.

## The agent's name

The agent's name is the `.md` file stem. Save the prompt as `hello.md` and the agent's name is `hello`. Discovery yields bare stems only, so a launch request can never be coaxed into naming a path.

The name follows the run everywhere it leaves a trace: the agent list, the session panel, and the persisted event log all key on it.

## The built-in chat and the shadow

A fresh install always offers a working chat agent, even when there is no agents directory at all. The built-in `chat` is a Markdown prompt embedded in the Workshop at compile time, and discovery always lists it.

Save your own prompt as `chat.md` in the agents directory and it shadows the embedded source: the list still shows one `chat`, but launching it runs your file. That is how your own agent takes over the chat role. An existing `chat.md` that cannot be read surfaces its error instead of silently serving the embedded source.

## The session surface

An agent prompt runs with two extras an unattached prompt does not have, both installed by the session. `user_input()` suspends the run until the operator types an answer, and returns the answer text together with an availability flag. `ui()` returns a fresh snapshot of host state on every call; its `selected_model` field names the model currently selected in the interface, so an agent that re-reads it each turn follows the operator's menu choice.

Everything else is the prompt language, exactly as the Prompt Language set teaches it: `models.infer` and `models.loop` run model rounds, `tools.add` brings tools into scope, `store` reads and writes files, `var` holds per-run state, and `log` records messages in the event stream.

## The moving parts

An agent run takes two products. The harness, reached through `harness-api`, owns discovery, launch, and the session extras: the input broker behind `user_input()`, the `ui()` snapshot, and the persisting run log. The `promptforge` crate is the unified runtime that parses and runs the prompt itself. The final chapter of this set walks through the built-in chat program, the one agent every install already has.

---

# The agent loop

This chapter teaches you the loop shape every agent follows. It is worth learning because the loop is how your program and the host talk: once you understand one request in flight, every host call reads the same way.

## The smallest loop

````lua
models.use('writer')

while true do
  local text = models.infer('Write one sentence about the sea.')
  log(text)
end
````

Run this program and it asks the model for one sentence about the sea, records the answer, and starts again. It keeps going until the host cancels the run.

Work through the lines. `models.use('writer')` selects the catalog model named `writer` for the run. `while true do` starts a loop with no exit of its own. `models.infer('Write one sentence about the sea.')` runs one direct, tool-free text completion on a fresh conversation and resumes with the completed text. `log(text)` records that text in the run's event stream.

## One request in flight

Each host call suspends your program with exactly one request in flight. When your program calls `models.infer`, it stops at that line and the host takes over. The host dispatches the one request and resumes your program at the same line with that request's answer as the return value.

Write your program as if each host call were an ordinary synchronous call. There is no callback and no second request to track: the program keeps exactly one request in flight, and it always resumes with that request's answer.

## The two round calls

Almost every agent is built from two calls. `models.chat(messages, opts)` runs one stateless model round over a message list your program builds, and the round is tool-capable. `tools.call(alias, args)` dispatches any tool in the agent's catalog by its wire name, and every tool in the catalog is in scope under its alias.

Both calls follow the loop rule: one request in flight, resumed with the answer. Everything else about them is detail on top of that rule.

## When the loop ends

A loop that never returns is legal. The host lets it run, and only the run's cancel flag stops it. Your program chooses its own shape: return when the work is done, or loop until the run is cancelled.

---

# Chat rounds

This chapter teaches you how your agent talks to a model. You will build a message list, run one round with `models.chat`, and read what the round produced. Your agent spends most of its life inside this call, so it pays to learn it exactly.

## A direct completion

````lua
models.use('writer')
local text = models.infer('Give this workshop a one-word name.')
log(text)
````

`models.infer(prompt)` runs one direct, tool-free text completion on a fresh conversation, and the call resumes with the completed text. Every call starts fresh: nothing persists from one `models.infer` call to the next.

Select the model first. `models.use('writer')` selects the catalog model named `writer`. An agent run has no default model, so a bare `models.infer` with no selection fails: "no model is selected: call models.use(...) before models.infer".

Select once. Your program selects only one model per run, and the runtime rejects a second selection.

## A chat round

````lua
local messages = {
  { role = 'system', content = 'You answer in one word.' },
  { role = 'user', content = 'What color is the sky?' },
}
local result = models.chat(messages)
log(result.reply)
````

`models.chat(messages, opts)` runs one stateless model round over the message list you build. Stateless means the list you pass is the whole conversation. You build it fresh, or you grow it yourself between rounds.

Build each message as a table with a `role` and a `content`. The `role` is one of `system`, `user`, `assistant`, or `tool`. The `content` is a plain string. Two more fields, `tool_call_id` and `tool_calls`, exist for rounds that involve tools. Any other fields you add are accepted and then dropped before the request is sent.

Get a role wrong and the error tells you where: the message names the 1-based index of the offending entry in your own list, as in `messages[2] role "wizard" is unknown`.

## The result table

````lua
local result = models.chat(messages)
if result.reply then
  log(result.reply)
end
````

A `models.chat` round returns a result table with five fields: `reply`, `tool_calls`, `finish_reason`, `model`, and `metrics`.

Read the outcome from `reply` and `tool_calls`. Exactly one of them is present: the round produced text, or it requested tool calls. Never both. When the round produced text, `result.reply` holds the completed text.

`result.model` names the model that served the round. `result.metrics` reports usage and backend timing. The `metrics` field is absent when nothing was measured, and absent optional fields read back as nil.

Check `finish_reason` for one thing: a value of "length" means the text reply was truncated.

Do not branch on `finish_reason` to detect tool calls. Some backends finish a tool-call round with "stop", and the calls still surface. Branch on the presence of `result.tool_calls` instead.

## Choose the model for one round

````lua
local result = models.chat(messages, { model = 'writer' })
````

`opts.model` chooses the chat model for one round. Pass a catalog model name. Without `opts.model`, the round uses your program's `models.use` selection. With neither, the call fails: "no model is selected: pass opts.model or call models.use(...) before models.chat".

Every model in the catalog is addressable by its catalog name, through `models.use` and through `models.get`. There is no default model in an agent run.

## A bound handle

````lua
local handle = models.get('writer')
log(handle.name)
log(handle.model_id)
local text = models.infer(handle, 'Write a haiku about rain.')
````

`models.get` addresses a catalog model by name and gives you a bound handle. `models.infer(handle, prompt)` runs the same kind of round as `models.infer(prompt)`: one direct, tool-free completion on a fresh conversation, using the handle's frozen binding. Handles are plain inspectable values with no methods; every operation that accepts one takes it as the leading argument.

The handle's fields are read-only. `name` is the prompt-local alias. `model_id` is the caller-facing catalog model id. `description` is the model's capability description. `context` is the catalog context window size in tokens. `thinking`, `temperature`, and `max_tokens` expose the invocation settings the handle's rounds send, and each reads nil when nothing sets it. Only the options table of `models.use` sets `temperature` and `max_tokens`, so they read nil on a `models.get` handle.

## Send an image

````lua
local messages = {
  { role = 'user', content = {
    { type = 'text', text = 'What does this sign say?' },
    { type = 'image_url', image_url = { url = 'data:image/png;base64,AA' } },
  } },
}
local result = models.chat(messages, { model = 'writer' })
````

Pass `content` as a non-empty array of content parts when a message mixes text and images. A content part has a `type` of `text` or `image_url`. An `image_url` part holds a data-URI, which sends the image to a multimodal model.

## Agent-only

`models.chat` exists only in an agent. The same call inside a document prompt fails as an undefined global.

---

# Tool calls

This chapter teaches you how your agent invokes tools. You will advertise tools to the model, read the calls the model asks for, dispatch them yourself, and answer them in the next round. Tools are how your agent reaches past the model, so the rules around them matter as much as the calls.

## Advertise tools to the model

````lua
local result = models.chat(messages, { tools = { 'echo' } })
````

List the tool aliases the model may ask for in `opts.tools`. The default is no tools. Pass no `tools` option and the round advertises none; the driver never adds any on its own.

## Read the requested calls

````lua
local result = models.chat(messages, { tools = { 'echo' } })
if result.tool_calls then
  for i = 1, #result.tool_calls do
    local call = result.tool_calls[i]
    log(call.name)
  end
end
````

A round with advertised tools can come back with requested tool calls. The requested calls arrive unexecuted. Running them is your program's decision, never the driver's.

Read each requested call from the 1-based entries of `result.tool_calls`. Each entry has three fields: `id`, `name`, and `arguments`. The `arguments` field is already a Lua table, so you can pass it straight on.

## Dispatch a call

````lua
local output = tools.call('echo')
````

`tools.call(alias, args)` dispatches any tool in your agent's catalog by its wire name. Every tool in the catalog is in scope under its alias. Omit `args`, or pass nil, to call a tool without arguments; the tool receives the empty argument object.

The call resumes with the tool's result. A tool that declares structured output returns its result as a Lua table. Every other tool returns plain text. A structured tool that returns invalid JSON fails the call, and the error names the alias.

## Answer a tool call

````lua
local result = models.chat(messages, { tools = { 'echo' } })
if result.tool_calls then
  local call = result.tool_calls[1]
  local output = tools.call(call.name, call.arguments)
  messages[#messages + 1] = { role = 'assistant', content = '', tool_calls = { { id = call.id } } }
  messages[#messages + 1] = { role = 'tool', tool_call_id = call.id, content = output }
  result = models.chat(messages, { tools = { 'echo' } })
end
````

The model asked for the call, so the next round must report what happened. Append two messages to your list. First replay the assistant's tool-calling round with an `assistant` message that holds the round's `tool_calls` array. Then answer the call with a `tool` role message whose string `tool_call_id` names the call it answers, with the tool's output as its content.

## Count the dispatches

````lua
tools.call('echo')
tools.call('echo')
if tools.calls['echo'] == 2 then
  log('echo ran twice')
end
````

The read-only `tools.calls` table shows how many times each alias has been dispatched in the run. The count increments on every attempted dispatch, even when the tool goes on to fail.

## Trust and the envelope

````lua
local wrapped = untrusted('a < b')
````

Tool output reaches your program under a trust flag. Trusted output passes to the next model turn or to your script verbatim. Untrusted output arrives inside an `<untrusted_input_...>` envelope, wrapped by the host before it can reach the next model turn or your script.

Mark your own strings with `untrusted(s)`. The call wraps the string in an envelope tagged with the run's guard nonce. The envelope has a fixed preface. Every literal `<` in the string is escaped as `&lt;`, so exactly one live open tag and one live close tag remain. Every `untrusted()` call in a run shares one nonce, so identical content produces a byte-identical envelope.

## No MCP tools yet

There are no MCP server tools yet. The mcp request shape is reserved.

---

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

Every event includes `provenance`, a table with `task` and `seq`. The `seq` value is the event's position within its task, and it only ever grows. Pass the highest `seq` you have already seen as `opts.last` and the call returns only later events, so a loop that runs once per turn reads each event exactly once. Store the cursor in `var` and it survives across sections.

## Reads stay deterministic

The log grows only when the run resumes from a host call, never in the middle of a chunk. The read is itself a host call: what `tasks.events` returns is fixed at the moment it returns, and no entry appears, moves, or changes inside the table afterwards. Two runs given the same inputs and the same answers see the same events in the same order.

## What an entry contains

Every entry has `kind`, `execution`, `section`, and `provenance`. The `kind` reads as a pinned snake_case label, such as `assistant_reply`, `tool_result`, `user_input`, `lua`, or `task_started`, and the rest of the table is that kind's own fields.

A model round leaves `assistant_reply` (with `turn`, `text`, `model`, `finish_reason`, `metrics`, and an `origin`) or `assistant_tool_calls` (with the requested `calls`); a reply's `origin` is `chat` for a user-facing turn and `infer` for a tool-free inference round, so a host that inspects `origin` can tell the two apart. A block of reasoning leaves `thinking`. Every dispatched tool call leaves `tool_call_succeeded` or `tool_call_failed`, and a call the model issued also leaves `tool_result` with `turn`, `tool_call_id`, `alias`, `content`, and `trusted`. Operator text arrives as `user_input`, and your own `log(...)` checkpoints as `lua` with `message`. Background work leaves `task_started` with its spawn seeds, then one of `task_succeeded`, `task_failed`, `task_cancelled`, or `task_abandoned`.

An absent optional field, such as a reply with no `finish_reason`, reads as nil, so test presence with a plain truth test.

## History across runs

A relaunched agent runs under a new task record, but the session's transcript persists: the harness writes every event to its run log before the next effect is issued, and a client reading the transcript sees every run the session has made, in order. Your program's own view through `tasks.events` covers the current run.

The `tasks` namespace is part of the prompt language, so the same call works in an unattached prompt. The session extras, `user_input()` and `ui()`, are what mark the agent environment.

---

# Host state

This chapter teaches you what state the host exposes to your agent and how to read it. Two globals hold it: `ui`, a live snapshot of host state, and `sys`, a sealed table. Knowing the difference keeps you from trusting a stale read or poking a table that pushes back.

## Read the UI snapshot

````lua
local snapshot = ui()
log(snapshot.selected_model)
````

`ui()` returns a fresh host-state snapshot. Every call re-queries the host. There is no caching, so two calls in a row can legitimately give you two different answers.

Read a field the host has not set and you get nil: a JSON null field in the snapshot reads as nil, never as a placeholder.

The `ui` global exists only when the host supplies a provider. When the host supplies none, the global is absent entirely. Check for its presence before you rely on it.

## The sealed sys table

The `sys` global is installed sealed and empty in an agent run. Any field read raises an error that names the field. Access fields by string key only; any other key type raises. The table is read-only, and its seal cannot be replaced from your code. A present-but-null field reads as nil rather than as a placeholder, but an agent run has no such fields, so every read still raises. In an agent run there is nothing behind the seal, so treat `sys` as off limits.

---

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
tools.call('user_input', {})
````

Request the operator's next message by invoking the `user_input` tool through `tools.call`. The operator's answer arrives in the event log as a `user_message` event, where your context building can read it.

---

# The sandbox

This chapter teaches you the limits the host enforces on your agent and what you can rely on inside them. Your code runs in a sandboxed Lua runtime, isolated from the host. The sandbox is what lets the host run author code safely, so it shapes every program you write.

## A fresh VM for every program

Each program runs in a fresh, restricted VM. Your file is compiled and run as Lua 5.5. Scripts execute isolated from the host: the only way in or out is a host call.

## What you have

Only the `string`, `table`, and `math` standard libraries are loaded, plus safe base functions. That is the whole standard library surface.

## What is removed

The code-loading and reflection globals are removed: `load`, `loadstring`, `dofile`, `loadfile`, `collectgarbage`, `require`, `getfenv`, `setfenv`, `rawget`, `rawset`, `rawequal`, `rawlen`, `print`, and `warn`. A script cannot load code dynamically, bypass the module system, or print directly.

The `io`, `os`, `package`, `coroutine`, and `debug` libraries are never loaded. Scripts have no file, OS, module-loading, or introspection access. File work goes through `store`, not `io`.

## Time and memory

Long-running and infinite loops are legal. The instruction budget is effectively unlimited, so a script is never killed for executing too many instructions. Only the run's cancel flag aborts a loop.

The VM runs under a Lua heap ceiling configured per run with `lua_memory_bytes`. The default is 64 MiB.

When your code trips a host quota, the error names the exhausted resource: "log event", "log byte", or "instruction".

## No direct yields

Your program cannot yield on its own. The `coroutine` global is stripped from author reach, and a hand-rolled yield fails the run: "scripts may not yield directly". Treat suspending host calls as ordinary synchronous calls, and let the host do the suspending.

---

# Errors and cancellation

This chapter teaches you what happens when a call fails or the run is cancelled, and how your agent responds. Most failures in this system are answers, not ambushes: they come back where the call was made, and your program chooses what to do next.

## Catch call failures with pcall

````lua
local ok, result = pcall(models.chat, messages, { model = 'writer' })
if not ok then
  log(result)
end
````

Wrap host calls in `pcall` to catch argument-validation and dispatch failures. These failures come back as the call's answer. They do not fail the run. A failed host call raises a Lua error whose text is exactly the host's message, so the value your `pcall` catches is the message the host sent.

## Errors that name things

The error messages are built to be read. A `tools.call` or an `opts.tools` entry that names an unregistered alias fails, and the error names the in-scope aliases. An `opts.model` outside the agent's catalog fails, and the error names the model. A bad chat message fails with the 1-based index of the offending entry in your own list, as in `messages[2] role "wizard" is unknown`.

A `models.chat` tool-call round fails when the model truncates it. Your program never resumes with a partial batch of tool calls. You get the failure instead.

## Runtime errors in your code

When your Lua code itself fails, the error names the location and the absolute line, prefixed as `{location}:{line}:`. In an agent run the location renders as ``agent `<name>` ``, with your agent's file stem as the name, so the message points back at your file.

## Cancellation

A host-fired cancel interrupts the run, even while a host call is suspended or Lua code is running. The run ends with the typed error "interrupted".

Every tool call is raced against the cancel signal. On cancel, the tool future is dropped, so a slow or stuck tool cannot hold the run. Your program does not see a partial tool result. The run simply ends.

---

# The full loop

This chapter assembles the complete agent: the built-in chat itself, one Markdown prompt embedded in the Workshop. A prompt saved as `chat.md` in the agents directory shadows it, so the chat you already use is a role your own agent can take. Walk through that program turn by turn, because the whole session surface shows up in it, working together.

## A chat agent

A chat agent is a transparent pass-through. It advertises no tools and sets no system prompt. It relays between the operator and the selected model, and nothing else. That restraint is the design: the program adds no behavior the operator did not ask for.

## One turn

The agent is one infinite loop in a single Lua block. Each turn does the same four things, in order.

1. Call `user_input()` to request the operator's next message, and return from the program when input is no longer available.
2. Append the operator's message to the retained history list.
3. Read the operator's selected model from the `ui()` snapshot's `selected_model` field.
4. Run `models.loop` over the history under that model, wrapped in `pcall`, then loop back to step 1.

## The full program

````markdown
---
name: chat
description: The built-in Workshop chat agent on the unified runtime.
promptforge: 0
---

# Chat

The built-in chat agent: a transparent pass-through between the operator
and the selected model. The message list is an explicit Lua value retained
across turns; the model is re-read from the host snapshot every turn, so a
menu selection change takes effect on the next turn.

## Conversation

```lua
local history = messages.new()
while true do
    local text, available = user_input()
    if not available then
        return
    end
    history:user(text)
    local selected = ui().selected_model
    if selected then
        pcall(function() return models.loop(models.get(selected), history) end)
    end
end
```
````

This is the whole chat surface. Work through the lines. `messages.new()` builds the empty conversation list once, before the loop starts. `user_input()` suspends the run until the operator answers, and returns the answer text together with an availability flag; when the flag reads false, the program returns instead of spinning on a dead session. `history:user(text)` appends the operator's message to the list. `ui().selected_model` reads the interface's current model selection, and `models.get(selected)` resolves that selection to a bound handle. `models.loop(handle, history)` runs the model round over the list, and the reply is appended to that same list as the final record, so the list the program passed in comes back one turn longer.

## Why the list is the state

Notice what the program never does: rebuild the conversation. The list is created once and retained across turns, and both sides accumulate in it - the program appends each operator message with `history:user(text)`, and `models.loop` appends each assistant reply as it completes. The next turn's model round therefore sees the whole conversation, and the program never copies, re-derives, or re-reads anything.

Because the model is re-read from the `ui()` snapshot on every turn, never captured once before the loop, a menu selection change takes effect on the very next turn.

## Why pcall wraps the model call

The loop runs `models.loop` under `pcall` because chat survives transport errors, and so must this program. A failed round does not kill the agent. The session surfaces the failure to the operator, and the loop returns to `user_input()` for the next turn.

## Grow from here

Start from this program and add one capability at a time. Bring a tool into scope with `tools.add` before the loop call and the model can ask for it. Save notes with `store.write`. Keep a counter in `var`. The loop does not change. The turns just do more.
