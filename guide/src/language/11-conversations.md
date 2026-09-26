# Conversations

`models.loop` runs a whole conversation with a model for you: it sends a message list round after round, runs the tools the model asks for, and appends every record back to your list until the model replies. This chapter shows how to build message lists and their records, what the loop appends, how the list is checked and sent to the model, and how to handle empty replies, context overflow, the round cap, and every failure the loop raises, so you can run multi-round model work and read back exactly what happened.

## A first conversation

This prompt asks the model for an explanation and returns its reply:

````markdown
---
name: explainer
description: Explains what a compiler does
promptforge: 0
models:
  writer: {}
---

# Explainer

```lua
models.default('writer')
```

## Explain

Explain what a compiler does in two short paragraphs.

```lua
local msgs = messages.new()
msgs:user(prose)
models.loop(msgs)
return msgs[#msgs].content
```
````

The block builds a conversation, runs it, and reads the reply back from the list. `messages.new()` creates an empty message list: a plain Lua table indexed by number, with length 0. `msgs:user(prose)` appends a user record, `{ role = "user", content = prose }`, holding the prose above the fence, and that text reaches the model as a user record. `models.loop(msgs)` runs the conversation, and `msgs[#msgs].content` is the model's reply.

A message list is what `models.loop` takes: a Lua array of records, each with the record's `role` set to `system`, `user`, `assistant`, or `tool`, plus a `content`. The loop runs a whole multi-round conversation over that list. Each round sends the list to the model, and when the model asks for tools, the loop runs them, appends the calls and their results to the list, and sends it again, so you never send single rounds or run the requested tools by hand.

When the model gives a text reply, the loop appends `{ role = "assistant", content = reply }` as the terminal record and returns. `models.loop` itself returns `nil`: everything the conversation adds lands in your list, in place, so once the loop returns, the reply is `msgs[#msgs].content`.

Each round runs on the section's current model: the role selected with `models.use`, or else the prompt-wide default set with `models.default`, as [Models](10-models.md#choosing-a-sections-model) explains. The loop looks that model up again each time it sends a round. With neither set, the call fails with the missing-model error, which is kind `internal` when caught with [`pcall`](05-lua-environment.md#catching-and-inspecting-errors) and ends the run with run error kind `Binding` when uncaught (see [how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)).

Builder methods are called with a colon and chain, each call appending one record at the end of the list, in call order. This block puts a system record ahead of the user record; `:system(content)` appends `{ role = "system", content = content }`:

````lua
local msgs = messages.new():system('Write for a reader who has never programmed.'):user(prose)
models.loop(msgs)
return msgs[#msgs].content
````

The list is an ordinary Lua array: `#msgs` is its length, `msgs[i]` reads a record, `msgs[#msgs]` reads the last one, `ipairs(msgs)` walks it, and each record's fields, such as `role` and `content`, read directly. This block collects each record's `role` after the loop:

````lua
local msgs = messages.new()
msgs:user(prose)
models.loop(msgs)

local roles = {}
for i, record in ipairs(msgs) do
  roles[i] = record.role
end
return #msgs .. ': ' .. table.concat(roles, ', ')
````

The prompt gives the model no tools, so the loop takes a single round, and the list ends with the user record and the terminal record:

````text
2: user, assistant
````

## Building message lists

The `messages` global exists in every Lua block of the prompt, the blocks in the H1 body included, and in the [shared library](03-blocks-and-prose.md#the-shared-library), because it is installed before the shared library is replayed. Its only member is `new`, and every list that `messages.new()` returns has five builder methods, each called with a colon:

| Method | Appends |
|---|---|
| `list:system(content)` | `{ role = "system", content = content }` |
| `list:user(content)` | `{ role = "user", content = content }` |
| `list:assistant(content, tool_calls)` | `{ role = "assistant", content = content }`, with `tool_calls` set only when it is passed |
| `list:tool(content, tool_call_id)` | `{ role = "tool", content = content, tool_call_id = tool_call_id }` |
| `list:append(record)` | `record` itself, unchanged |

Every builder method, `append` included, changes the list and returns that same list, so builder calls chain to any length. This block puts a worked example, one user record and one assistant record, ahead of the real request:

````lua
local msgs = messages.new()
  :system('Translate each phrase into French.')
  :user('good morning')
  :assistant('bonjour')
  :user(prose)
models.loop(msgs)
return msgs[#msgs].content
````

Because each call changes the list itself, records can also be added in separate statements or inside a loop:

````lua
local reviews = { 'Great battery life.', 'The screen scratches easily.', 'Fast shipping.' }
local msgs = messages.new():system('Rate each review from 1 to 5, one line per review.')
for _, review in ipairs(reviews) do
  msgs:user(review)
end
models.loop(msgs)
return msgs[#msgs].content
````

`list:assistant(content, tool_calls)` appends an assistant record. Its `tool_calls` argument is optional, and each tool call in it is a table `{ id = ..., name = ..., arguments = {...} }`. `list:tool(content, tool_call_id)` appends a tool record answering one of those calls. Together they write an earlier tool call and its result into the list:

````lua
local msgs = messages.new()
  :user('What is the weather in Paris?')
  :assistant('', { { id = 'call_1', name = 'weather', arguments = { city = 'Paris' } } })
  :tool('Sunny, 24 C', 'call_1')
  :user('And in Lyon?')
````

`list:append(record)` appends a record you built yourself, unchanged and with any extra fields; fields the loop does not read are ignored. Builder calls and hand-written records mix freely, and the builders are optional: a hand-written array of records passed to `models.loop` is checked exactly the same way.

````lua
local msgs = {
  { role = 'system', content = 'Answer in one sentence.' },
  { role = 'user', content = prose },
}
models.loop(msgs)
return msgs[#msgs].content
````

A built list stays plain data: its length, its numeric indexes, and its conversion to JSON contain only the records, because the builder methods live on the list's metatable, never as fields. Message lists are also the only values the host provides that have colon-call methods. [Model handles](10-models.md#model-handles) have read-only fields and no methods, and no other handle the host provides has methods either, so you pass handles to functions instead, as in `models.infer(handle, prompt)`.

## Message records

Every record is a Lua table holding the record's `role` and a `content`, plus `tool_calls` or `tool_call_id` on the records that carry them:

| The record's `role` | Fields |
|---|---|
| `system` | `content` |
| `user` | `content` |
| `assistant` | `content`, plus `tool_calls` when the record holds tool calls |
| `tool` | `content` and `tool_call_id` |

The record's `role` is one of the exact lowercase strings `system`, `user`, `assistant`, or `tool`. A record's `content` is either a string or a non-empty array of content parts. Plain-text content is a string, and the empty string is allowed.

Multimodal content is a non-empty array of content parts, and one record can mix text parts and image parts. A text part is `{ type = "text", text = "..." }`. An image part is `{ type = "image_url", image_url = { url = "data:image/png;base64,..." } }`; the check requires a string `url` inside the `image_url` table and ignores any other keys there.

````lua
local msgs = messages.new()
msgs:user({
  { type = 'text', text = 'What does this chart show?' },
  { type = 'image_url', image_url = { url = 'data:image/png;base64,iVBORw0KGgo...' } },
})
````

An assistant record lists the tool calls it made in `tool_calls`: an array of tables, each with a string `id`, a string `name`, and an optional `arguments` table. A call without `arguments`, or with `arguments = nil`, gets an empty arguments table, `{}`. One assistant record can carry visible reply text and several tool calls together.

Each tool call is answered by its own tool record, whose string `tool_call_id` equals the call's `id`. Every tool record sets `tool_call_id`, and `tool_call_id` belongs on tool records only.

````lua
msgs:append({
  role = 'assistant',
  content = 'Let me check both.',
  tool_calls = {
    { id = 'call_1', name = 'weather', arguments = { city = 'Paris' } },
    { id = 'call_2', name = 'clock' },
  },
})
msgs:tool('Sunny, 24 C', 'call_1')
msgs:tool('14:05', 'call_2')
````

The second call sets no `arguments`, so it gets `{}`.

Extra fields on records, content parts, or tool calls are accepted and dropped before the model sees the list. Every value in them must still be JSON-representable: strings, numbers, booleans, and nested tables.

## What the loop appends

When tools are [in scope](12-tools.md#advertising-tools-to-the-model) for the section, a round can end with the model asking for tools instead of replying, and such a round is a tool round. After one tool round that calls one tool, followed by a text reply, the list holds four records:

````lua
{
  { role = 'user', content = 'Will it rain in Paris tomorrow?' },
  { role = 'assistant', content = '', tool_calls = {
    { id = 'call_1', name = 'weather', arguments = { city = 'Paris' } },
  } },
  { role = 'tool', content = 'Rain likely, 12 C', tool_call_id = 'call_1' },
  { role = 'assistant', content = 'Yes, rain is likely in Paris tomorrow.' },
}
````

The loop runs a round's [model tool calls](12-tools.md#model-tool-calls) one after another in call order, then appends the whole batch at once: one assistant record holding every call in order, followed by one tool record per call in the same order. A round that calls two tools therefore adds three records.

Each call in the assistant record's `tool_calls` has an `id`, a `name`, and `arguments` already parsed into a Lua table, so `msgs[2].tool_calls[1].arguments.city` reads `'Paris'` directly. The `name` is the [alias](12-tools.md#tool-slots-and-tool-objects) the model called: the model calls a tool by its alias from `tools:`, never by its tool path.

Each result is a tool record placed after the round's assistant record, with `tool_call_id` equal to the call's `id` and `content` holding the tool's result text.

When a bound tool fails during the loop, the loop does not stop: the tool's failure text becomes that call's tool record, and the call still counts as answered. A [local tool](12-tools.md#local-tools) handler that the model calls runs before the round's batch is appended, so if it reads the message list it sees the list as it stood before the round began; an error it raises is raised again at the `models.loop` call with its own kind and never becomes failure text.

A tool record's `content` is exactly the text the model receives for that call, so when a result or a failure text reaches the model inside the [untrusted envelope](09-the-store.md#wrapping-untrusted-text), the envelope is part of the record too.

Every call the loop records carries a non-blank `id` unique within its round, a non-blank `name`, and `arguments` as a decoded table, never JSON text. A model reply that breaks these rules, with a blank id or name, an id repeated within the round, or arguments that are missing or do not decode to a JSON object, fails the round as a malformed reply, raised at the call as kind `internal`. Ids must also be unique across the whole list, so a model that repeats an id from an earlier round makes the next round fail with `messages[{n}] tool call id "{id}" duplicates an earlier tool call`. The `name` comes from the model's reply and need not be [in scope](12-tools.md#advertising-tools-to-the-model); a name outside the round's scope is the `out_of_scope_tool` failure.

Every round sends the whole list so far, so the model sees all earlier tool calls and results when it replies.

The loop appends to the list you passed in, so the list stays usable after `models.loop` returns. It then holds the whole conversation in order: the user record, each tool round's assistant record and tool records, any [task notices](15-tasks.md#task-notices-to-the-model), which arrive as user records with any task result they embed inside the untrusted envelope, and the terminal record. Every record has a `role` and `content`, and each tool record's `tool_call_id` equals the id of the call it answers. One user record, one tool round with one call, and the reply make 4 records; with four such tool rounds they make 10.

One list can serve several `models.loop` calls. Appending a new user record between calls lets each loop continue the same conversation, and each call appends its own terminal record:

````lua
local msgs = messages.new()
msgs:user('Name three sorting algorithms.')
models.loop(msgs)
msgs:user('Which of those is stable?')
models.loop(msgs)
return msgs[#msgs].content
````

With no tool rounds, the list ends with four records: user, assistant, user, assistant. The loop always leaves its terminal record in the list; when you do not want it kept, remove it with `msgs[#msgs] = nil`.

## Model and tool scope

The full form of the call is `models.loop(handle?, messages, compactor?)`: an optional model handle first, then the message list, and last an optional compactor, which decides what happens when a round overflows the model's context window. A handle passed first, from [`models.get`](10-models.md#model-handles), pins the loop to that handle's model: every round of such a call, at any point in the section, runs on the handle's model instead of the section's current model. This prompt drafts on the default role and reviews on a second one:

````markdown
---
name: second_opinion
description: Drafts a text, then has a second role review it
promptforge: 0
models:
  writer: {}
  reviewer: {}
---

# Second Opinion

```lua
models.default('writer')
```

## Draft and review

Write a two-sentence product description for a paper notebook.

```lua
local draft = messages.new()
draft:user(prose)
models.loop(draft)

local review = messages.new()
review:user('List any factual or grammar errors in this text: ' .. draft[#draft].content)
models.loop(models.get('reviewer'), review)
return review[#review].content
```
````

`models.loop` tells the forms apart by type: a userdata first argument is taken as the handle, and any other first argument as the message list. The handle is checked before the list, so a bad handle is reported first: a userdata that is not a model handle fails with a `lua`-kind error, `models.loop handle must be a model handle`.

Each round offers the model the tools [in scope](12-tools.md#advertising-tools-to-the-model) for the section at the moment that round is sent, [local tools](12-tools.md#local-tools) included: a loop with no tools in scope takes a single round and offers the model no tools, and a tool added with `tools.add` between two loops is offered only to the later loop.

The model's [sampling options](10-models.md#sampling-options) apply to every round of the loop: each round is sent with the options of the model it runs on, the conversation so far, and the tools on offer.

[`sys.model`](10-models.md#the-bound-model-in-sysmodel) becomes readable only after the section's first tool call: a tool call the model makes inside the loop counts, but a loop round with no tool call does not make it readable.

## Turns and live output

The round count advances once for every round that returns a reply, whether a text reply or a batch of tool calls, and an empty reply advances it too; a round that overflows the model's context window, and a round that fails, do not. A [task](15-tasks.md#starting-a-task) counts its rounds against its own round count, which its status table shows as the `turns` field. The round count is separate from the round cap, the most rounds a single `models.loop` call may make, which each call counts for itself.

The run reports each loop round and each tool call as an event filed under the section that ran the loop, and [round events](16-task-events.md#model-round-events) carry the round count as their `turn` field; a round that calls one tool, followed by a text reply, reports these in order:

````text
model_turn_completed
tool_call_succeeded
model_turn_completed
````

When the host shows live output, each text fragment of a loop round reaches the host as it arrives, in the order the model streamed it, and the reply in the terminal record is those fragments joined. A [`models.infer`](10-models.md#running-a-round-with-modelsinfer) round does not stream. A host that stops listening does not fail the round.

## How the list reaches the model

Right before every round is sent, the loop composes the list for the model from whatever records it holds at that moment, so a list edited between calls is checked and composed again each time. A well-formed list, with a leading system record and then alternating user and assistant records, reaches the model exactly as written, and other lists reach it with runs of records merged.

Sending a list never changes it: composing and merging happen on a copy, so the list stays the conversation's running state, and `#msgs` counts records as appended, not the merged records the model receives. This list opens with two system records and ends with two user records:

````lua
local msgs = messages.new()
  :system('You are terse.')
  :system('Answer in French.')
  :user('Hello.')
  :user('What time is it?')
models.loop(msgs)
````

The model receives two records:

````lua
{
  { role = 'system', content = 'You are terse.\n\nAnswer in French.' },
  { role = 'user', content = 'Hello.\n\nWhat time is it?' },
}
````

After the loop, with no tool rounds, `#msgs` is 5: the four records as written plus the terminal record. These are the merge rules:

| Records in the list | What the model receives |
|---|---|
| Two or more leading system records | One system record, their texts joined by a blank line |
| Two or more user records in a row | One user record, their texts joined by a blank line |
| Two or more text-only assistant records in a row | One assistant record, their texts joined directly, with no separator |
| Assistant text right before an assistant tool-call record | One assistant record carrying both the text and the calls |
| Tool records | Never merged; each tool result stays separate |

Because user records merge, you never merge them yourself to satisfy a provider's alternation rule, and text-only assistant records in a row, such as the fragments of one reply, reach the model as one assistant record.

Merging stays clean. When both merged records are plain text, an empty one adds nothing, so no stray blank line appears. When either uses content parts, the result is a parts array in arrival order, with the separator (a blank line between user records, nothing between assistant records) inserted as its own text part, even when the other side is empty text.

System records, when present, open the list, and they belong only in that leading block; a system record after any other record fails the call with `messages[{n}] is a system message outside the leading system block`, naming the misplaced record. A lone leading system record may use plain text or content parts, but when the list opens with two or more system records, each of them is plain text; otherwise the call fails with `messages[{n}] is a system message with content parts; only plain text system messages can be composed`, naming the first such record. That check runs before every other rule that spans records, so it is the error reported even when later records also break one of those rules.

Text and images go in one record as a content array of text parts and image parts; the model receives each part in order, as text or as an image referenced by its URL.

Earlier tool use replays: assistant records whose calls carry `id`, `name`, and structured `arguments` reach the model as its own earlier function calls, with the arguments sent as JSON text. The loop records calls in the same shape you write them, so its records and yours replay the same way.

Only a record's `role`, `content`, `tool_call_id`, and `tool_calls` are sent to the model. Nothing else a record holds, such as a copied credential, reaches the provider.

## Checking the list

Mistakes in the message list are caught where `models.loop` is called. Each raises a `lua`-kind error value at that call, catchable with `pcall`, whose message gives the 1-based position of the bad record, content part, or tool call. The builder methods check nothing, so a malformed record fails at the `models.loop` call, not at the builder call, with the same error a hand-written array gives:

````lua
local msgs = messages.new():user()
local ok, err = pcall(models.loop, msgs)
return tostring(err)
````

````text
messages[1] content must be a string or a non-empty array of content parts
````

The whole list is checked at the `models.loop` call and again on every round, right before that round is sent, so records you or the loop appended between rounds meet the same rules. Each check has two layers: first each record's own fields and their types, then the rules that span records, covering system placement, where `tool_calls` and `tool_call_id` may appear, unique call ids, and the pairing of every call with its result. Both layers raise at the `models.loop` call.

Pairing works by batch. Right after an assistant record with `tool_calls` come its tool records, one per call, each with a `tool_call_id` matching one call's `id`. They may come in any order within the batch, and nothing else comes between the assistant record and its last tool record. The loop itself always appends a complete batch.

In a record error, `messages[{index}]` is the 1-based Lua position of the offending record, and the first bad record in list order is the one reported. In a part error, `content part {part_index}` is the 1-based position of the part within that record's content array, and the first bad part is the one reported. In a call error, `tool_calls[{call_index}]` is the call's 1-based position.

### The list as a whole

| Rule | Message when it breaks |
|---|---|
| The message list is a Lua table | `messages must be a table of message tables, got {type}`, naming the Lua type received, which is `nil` when the list is missing |
| Every value is a string, number, boolean, or nested table, even in fields the check otherwise ignores | `messages must be a JSON-representable table` |
| The list holds at least one record | `messages must not be empty` |
| The list is a sequence of records | `messages must be an array of message tables` |

### Each record

| Rule | Message when it breaks |
|---|---|
| Each record is a table | `messages[{index}] must be a message table` |
| The record sets its `role` as a string | `messages[{index}] role must be a string, one of: system, user, assistant, tool` |
| The record's `role` is one of the four | `messages[{index}] role "{role}" is unknown; known roles: system, user, assistant, tool`, quoting the string |
| The record sets `content` as a string or a non-empty array of parts | `messages[{index}] content must be a string or a non-empty array of content parts` |
| Each part is a table with a string `type` | `messages[{index}] content part {part_index} must be a table with a string type field` |
| A part's `type` is `text` or `image_url` | `messages[{index}] content part {part_index} has unknown type "{type}"; known types: text, image_url` |
| A text part sets a string `text` | `messages[{index}] content part {part_index} is a text part and must set a string text field` |
| An image part sets an `image_url` table with a string `url` | `messages[{index}] content part {part_index} is an image_url part and must set an image_url table with a string url field` |
| `tool_call_id`, when present, is a string | `messages[{index}] tool_call_id must be a string` |
| A tool record sets `tool_call_id` | `messages[{index}] is a tool message and must set a string tool_call_id` |
| `tool_calls`, when present, is an array | `messages[{index}] tool_calls must be an array` |
| Each tool call is a table | `messages[{index}] tool_calls[{call_index}] must be a table` |
| Each tool call sets a string `id` | `messages[{index}] tool_calls[{call_index}] must set a string id` |
| Each tool call sets a string `name` | `messages[{index}] tool_calls[{call_index}] must set a string name` |
| A tool call's `arguments` is a keyed table | `messages[{index}] tool_calls[{call_index}] arguments must be a table` |

The type check on `tool_call_id` comes first, so a tool record whose `tool_call_id` is not a string reports `messages[{index}] tool_call_id must be a string`. A tool call's `id` is checked before its `name`, so a call missing both reports the `id` message. For `arguments`, any value other than a keyed table, a non-empty sequence included, breaks the rule.

### Rules across records

| Rule | Message when it breaks |
|---|---|
| A leading block of two or more system records is plain text | `messages[{n}] is a system message with content parts; only plain text system messages can be composed`, naming the first such record |
| `tool_calls` appears on assistant records only | `messages[{n}] sets tool_calls but is not an assistant message`, naming that record |
| `tool_call_id` appears on tool records only | `messages[{n}] sets a tool_call_id but is not a tool message`, naming that record |
| System records appear only in the leading block | `messages[{n}] is a system message outside the leading system block`, naming the misplaced record |
| Every tool call `id` is unique across the whole list | `messages[{n}] tool call id "{id}" duplicates an earlier tool call`, naming the assistant record that repeats it |
| Every call in a batch gets its tool record before any other record arrives or the list ends | `messages[{n}] tool call "{id}" has no tool result`, naming the assistant record and its first unanswered call in call order |
| Each tool record answers a still-pending call from the batch just before it, exactly once | `messages[{n}] is an orphan tool record: no pending assistant tool call with id "{id}"`, naming the tool record and its id |

Ids are unique across the whole list, not only within one assistant record: reusing an id from any earlier record, or twice in one record, breaks the rule. That covers the ids the loop appends for the model too, so a model that repeats an id from an earlier round makes the next round fail this way.

The rules across records run in this order: first the plain-text check on a leading block of two or more system records; then each record in list order, with the `tool_calls` placement rule, then the `tool_call_id` placement rule, then the rules for its own `role`; and last the check for a batch still open at the end of the list. The checks on each record's own fields run before all of them.

All of these are `lua`-kind error values raised at the `models.loop` call and catchable with `pcall`. Each message is exactly the text shown, with no prefix, and only the first broken rule is reported. A failed check across records is also reported as a `model_turn_failed` [round event](16-task-events.md#model-round-events) before the error reaches the call.

## Empty and truncated replies

A round's reply comes with a finish reason when the provider sends one, such as `"stop"`, or `"length"` for a reply the provider cut short for length. A reply that is empty or only whitespace counts exactly as no reply, and one rule decides what happens to it.

The clean exit: after at least one tool call in the loop has been answered, the model may finish with an empty reply and finish reason `"stop"`. The loop accepts this, appends an empty assistant record, `{ role = "assistant", content = "" }`, as the terminal record, and returns.

Every other empty reply raises `empty_model_reply`: an empty `"stop"` reply when no tool call was made in the loop, whether or not the model had tools to call; any empty `"length"` reply; and an empty reply with no finish reason, even after tool calls.

`pcall` catches it as an error value whose `kind` is `"empty_model_reply"`, whose `finish_reason` field holds the finish reason when the provider sent one, and whose message is a detail phrase about the empty reply, or plain `empty model reply` when there is none. The rejected round appends nothing to the list:

````lua
local msgs = messages.new()
msgs:user(prose)
local ok, err = pcall(models.loop, msgs)
if not ok then
  return err.kind .. '|' .. tostring(err.finish_reason) .. '|' .. tostring(err)
end
return msgs[#msgs].content
````

When the model's first reply is empty, with finish reason `"stop"` and no detail phrase, `#msgs` stays 1 and the result is:

````text
empty_model_reply|stop|empty model reply
````

Reasoning text from the model is never used as the reply; the detail may say it was ignored, as in `empty model reply: reasoning content was present but ignored`. Left uncaught, `empty_model_reply` ends the run with run error kind `Completion` (see [how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)).

An empty reply still completes its round: it advances the round count, and the loop then either takes the clean exit or raises `empty_model_reply` with the reply's detail as the message and, when the provider sent one, the finish reason in `finish_reason`. A text reply that the provider cut short for length, with finish reason `"length"`, is accepted as the terminal record.

Both show up as [round events](16-task-events.md#model-round-events): an empty reply is reported as `model_turn_completed`, carrying the empty-reply detail and finish reason, and a truncated text reply reports `model_turn_truncated` right after its `model_turn_completed`, as a truncated `models.infer` round does too.

## Compactors and context exhaustion

A compactor is the policy for a round that overflows the model's context window. It is the optional last argument of `models.loop`: second without a handle, as in `models.loop(msgs, compactor)`, and third with one, as in `models.loop(handle, msgs, compactor)`. The `compactors` global, available in all of a prompt's Lua code and installed alongside `messages`, is a table of the shipped policies. Leaving the argument out selects `compactors.fail`, the only shipped policy, so `models.loop(msgs)` and `models.loop(msgs, compactors.fail)` behave the same.

`compactors.fail` never compacts: an overflowing round raises `context_exhausted`. A round overflows for one of two reasons:

| Reason | What happened |
|---|---|
| `precheck` | The estimated request size exceeded the model's context window, and no request left the host |
| `provider` | The provider rejected the request as too large for its context window |

`pcall` catches context exhaustion as an error value whose `kind` is `context_exhausted` and whose `reason` field is `"precheck"` or `"provider"`:

````lua
local msgs = messages.new()
msgs:user(prose)
local ok, err = pcall(models.loop, msgs)
if not ok then
  return err.kind .. ': ' .. tostring(err.reason)
end
return msgs[#msgs].content
````

When the prose is far longer than the model's context window, the round is refused before any request leaves the host, and the result is:

````text
context_exhausted: precheck
````

The message is `context exhausted: ` followed by the reason in words:

````text
context exhausted: the request precheck overflowed the model's context window
context exhausted: the provider rejected the request as exceeding the model's context window
````

Left uncaught, context exhaustion ends the run with run error kind `ContextExhausted` (see [how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)).

A compactor can also be your own function. When a round overflows, the loop calls it instead of raising, with the reason `"precheck"` or `"provider"` as its single string argument. A custom compactor ends by raising an error, which propagates out of `models.loop` unchanged, a bare string staying a bare string, so `pcall` around the loop catches it:

````lua
local ok, err = pcall(models.loop, msgs, function(reason)
  error('the notes are too long to send (' .. reason .. ')', 0)
end)
if not ok then
  return err
end
return msgs[#msgs].content
````

````text
the notes are too long to send (precheck)
````

A compactor that returns at all, with or without a value, makes `models.loop` raise a `lua`-kind error whose message names `compactors.fail`. Left uncaught in a walked section, a compactor's own error ends the run with run error kind `Lua` (see [how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)), with the compactor's error text in the message.

`compactors.fail(tag)` takes one reason string and raises a `context_exhausted` error value whose `reason` is that string; it never returns. The tag is exactly `"precheck"` or `"provider"`; any other string raises a `lua`-kind error, `unknown overflow reason "{tag}"; expected "precheck" or "provider"`, which is an authoring error rather than context exhaustion.

The compactor is a function value. Any other value makes `models.loop` raise a `lua`-kind error, `compactor must be a function, got {type}`, before any request is sent, where `{type}` is `integer` for an integer, `number` for a float, and otherwise the Lua type name.

A refused round appends nothing. A bad compactor argument is refused before any round, so the list is untouched; a round refused for overflow, or for a compactor that returns, adds no records; and records from rounds that already completed stay in the list.

### The precheck

Before each round is sent, a precheck compares the estimated request size with the model's context window. Only an estimate larger than the window is refused, with reason `"precheck"` and before anything leaves the host, so an estimate equal to the window passes. A handle that [`models.get`](10-models.md#model-handles) returns for a raw model id, rather than for a declared role, has a context window of 8192 tokens for this check.

The estimate is the conversation's total text length in UTF-8 bytes, divided by 4 with the remainder dropped, plus 4 tokens for each record sent. The division runs once over the whole conversation, and records are counted after they merge as [How the list reaches the model](#how-the-list-reaches-the-model) describes, so merged records pay the per-record overhead once. One 396-character record estimates to 103 tokens, 99 for the text plus 4, so a 103-token window admits it and a 102-token window refuses it. Non-ASCII text weighs more per visible character, because each such character takes more than one byte.

The estimate counts a record's plain string content, the `text` of each text part, and the whole serialized form of each tool call the record carries, not only its arguments. Image parts count nothing, so an image-heavy conversation can pass the precheck and still overflow at the provider.

### Provider rejections

A rejection from the provider counts as reason `"provider"` when it has HTTP status 400 or 413 and its body contains, ignoring case, one of these phrases: `context length`, `context window`, `context size`, `context_length_exceeded`, `too many tokens`, or `prompt is too long`. The loop then calls the compactor with reason `"provider"`, after that one request.

Every other backend failure, such as any 5xx status, any status other than 400 or 413, or a 400 or 413 whose body has none of those phrases, stays an ordinary backend failure with no compactor call: `models.loop` raises it at the call as an `internal`-kind error.

## The round cap

The round cap is the most rounds a single `models.loop` call may make. `max_tool_iterations:` in the [frontmatter](02-file-structure.md#frontmatter-rules-and-errors) sets it for the prompt:

````yaml
name: researcher
description: Answers a question with a small round cap
promptforge: 0
max_tool_iterations: 5
````

The value is a whole number from 1 through 1000, and it overrides the run's default for this prompt. Without `max_tool_iterations:`, the run's default applies: 24 rounds per `models.loop` call, unless the host running the prompt sets a different default, which a prompt's own value also overrides. Each `models.loop` call gets the full cap on its own, so two loops in one section can each make that many rounds.

Against a model that keeps calling tools, `models.loop` with a round cap of N makes exactly N rounds and then fails with `tool_loop_exhausted`. `pcall` catches it as an error value whose `kind` is `tool_loop_exhausted` and whose `tostring(err)` is `tool-call loop did not converge`, with no extra fields. Left uncaught, it ends the run with run error kind `Tool` (see [how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)).

A model that keeps calling a failing bound tool reaches the cap the same way, because each failure is answered with failure text and the model never gives a text reply. After exhaustion the list holds only complete rounds: every round appended its assistant tool-call record and a tool record for each call, with no half-answered batch.

````lua
local msgs = messages.new()
msgs:user(prose)
local ok, err = pcall(models.loop, msgs)
if ok then
  return msgs[#msgs].content
end
return tostring(err) .. ' after ' .. #msgs .. ' records'
````

With `max_tool_iterations: 2` and a model that calls one tool in every round, the list ends with the user record plus two rounds of two records:

````text
tool-call loop did not converge after 5 records
````

A value out of range fails at parse, before the run starts, with parse error kind `Frontmatter` (see [parse error kinds](17-limits-and-errors.md#parse-error-kinds)). Zero or less gives `max_tool_iterations must be a positive integer (>= 1), got {raw}`, and more than 1000 gives `max_tool_iterations must be <= 1000, got {raw}`. `{raw}` echoes the value as written, and very large values are range-checked, never wrapped.

## Catching loop failures

Every failure of `models.loop` is raised at the call, so one `pcall` pattern covers them all. `pcall` returns an error value with a `kind` field, fields for that kind, and a readable message through `tostring(err)`:

````lua
local msgs = messages.new()
msgs:user(prose)
local ok, err = pcall(models.loop, msgs)
if ok then
  return msgs[#msgs].content
elseif err.kind == 'tool_loop_exhausted' then
  return 'No final reply within the round cap.'
elseif err.kind == 'context_exhausted' then
  return 'The request is too long for the model (' .. err.reason .. ').'
end
error(err)
````

The loop raises these error kinds:

| Error kind | Raised when | Uncaught, the run ends as |
|---|---|---|
| `lua` | An argument or the message list breaks a rule, or a compactor returns instead of raising | `Lua` |
| `empty_model_reply` | An empty reply is not the clean exit; `finish_reason` holds the finish reason | `Completion` |
| `context_exhausted` | A round overflows under `compactors.fail`; `reason` is `"precheck"` or `"provider"` | `ContextExhausted` |
| `tool_loop_exhausted` | The round cap runs out; there are no extra fields | `Tool` |
| `out_of_scope_tool` | The model calls a tool that is not [in scope](12-tools.md#advertising-tools-to-the-model) for the round; `name` holds the requested name | `Tool` |
| `internal` | A backend, transport, or other host-side failure, the missing-model error included | `Completion` for backend and transport failures, `Binding` for the missing-model error |

An `out_of_scope_tool` message begins `tool "{name}" is not in this section's scope; in-scope aliases: [{aliases}]`, where `{aliases}` lists the aliases in scope, each in double quotes, as in `["echo"]`, and none of that round's calls run, so the round appends nothing. An error raised inside a [local tool](12-tools.md#local-tools) handler during the loop reaches the call with its own kind.

`models.loop` takes at most three arguments with a handle and at most two without one; more raise a `lua`-kind error, `models.loop takes (handle?, messages, compactor?)`.

Any failure while preparing a round, whether in choosing the model, reading the section's tools, checking the list, or reaching the provider, is raised at the `models.loop` call like any other call error, so `pcall` catches it. Any other failed round, such as a backend error that is not a context-window rejection, is reported as a `model_turn_failed` [round event](16-task-events.md#model-round-events) and raised as the call's error, with kind `internal` for backend and transport failures.

The last column of the table is the run error kind when a loop failure in a walked section is left uncaught (see [how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)). An error value you catch and raise again before the block makes another [suspending call](05-lua-environment.md#calls-that-wait-and-errors-that-raise), as the `error(err)` line above does, ends the run the same way. Raised again after another suspending call, it keeps its run error kind for `context_exhausted` (with its `reason`), `tool_loop_exhausted`, `empty_model_reply`, and `tool`, a `cancelled` value ends the run with the cancelled outcome rather than as a failure, and any other kind ends the run as `Lua`. In the [H1 pass](04-how-a-prompt-runs.md#the-h1-pass), an ordinary Lua error that would end the run as `Lua` ends it as `RequirementsUnmet` instead, whose notice is the Lua error text, and the other kinds keep their run error kind.
