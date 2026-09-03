# Chat rounds

This chapter teaches you how your agent talks to a model. You will build a message list, run one round with `models.chat`, and read what the round produced. Your agent spends most of its life inside this call, so it pays to learn it exactly.

## A direct completion

````lua
models.use('writer')
local text = models.infer('Give this workshop a one-word name.')
log(text)
````

`models.infer(prompt)` runs one direct, tool-free text completion on a fresh conversation, and the call resumes with the completed text. Every call starts fresh: nothing carries over from one `models.infer` call to the next.

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

`result.model` names the model that served the round. `result.metrics` carries usage and backend timing. The `metrics` field is absent when nothing was measured, and absent optional fields read back as nil.

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
local text = handle:infer('Write a haiku about rain.')
````

`models.get` addresses a catalog model by name and gives you a bound handle. `handle:infer(prompt)` runs the same kind of round as `models.infer`: one direct, tool-free completion on a fresh conversation, using the handle's frozen binding. Pass no second argument. `handle:infer(prompt)` takes none, and passing one is an explicit error.

The handle's fields are read-only. `name` is the prompt-local alias. `model_id` is the caller-facing catalog model id. `description` is the capability description given at bind time. `context` is the catalog context window size in tokens. `thinking`, `temperature`, and `max_tokens` expose the frozen invocation settings, and they read nil when the bind declared none.

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

Pass `content` as a non-empty array of content parts when a message mixes text and images. A content part has a `type` of `text` or `image_url`. An `image_url` part carries a data-URI, which sends the image to a multimodal model.

## Agent-only

`models.chat` exists only in an agent. The same call inside a document prompt fails as an undefined global.

