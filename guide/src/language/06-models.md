# Models

A prompt does not name a model directly. It describes the capability it needs, and the runtime resolves that description against the catalog. This chapter teaches the three calls that declare and select models, `models.bind`, `models.default`, and `models.use`, plus the two operations that run model rounds from Lua, `models.infer` and `models.loop`. Capability-based binding is what keeps a prompt portable across catalogs, so it is worth learning as a habit from the start.

## Binding a model

Declare a model alias in the preamble with `models.bind`:

````lua
models.bind('analyst', 'careful analysis', { temperature = 0.25, max_tokens = 64, thinking = false })
````

The first argument is the local alias, the second is a natural-language capability description, and the third attaches invocation options such as `temperature`, `max_tokens`, `thinking`, and `context`. The options freeze at bind time and ride on every request that uses the binding.

## The default model

The call `models.default` designates the prompt-wide default, and it comes in two forms. The multi-argument form binds and designates in one call:

````lua
models.default("writer", "A tiny model", { thinking = false, temperature = 0 })
````

The single-argument form designates an already-bound alias: `models.default("writer")`. The two forms cannot be combined, and `models.default` may be called at most once per prompt, only during the live H1 pass.

## Selecting a model for a section

Inside a section, `models.use('analyst')` selects a bound alias for that section. The selection is read when a model round starts, so a later `models.use` call in the same section replaces it and steers the next round. A section that runs a model round needs a model from `models.use` or from the prompt-wide default; with neither, the call fails with a model-required error.

## Inspecting a binding

The call `models.get(alias)` returns an inspectable handle with `name`, `model_id`, `description`, `context`, `thinking`, `temperature`, and `max_tokens` fields. Reading a handle does not change the section's selection. Handles are plain values: they have no methods, and every operation that accepts one takes it as a leading argument.

## Direct inference

Sometimes you want one quick model round over a prompt string, most often the section's `prose`. The call `models.infer(prompt)` runs one tool-free inference round on the section's current model, and `models.infer(handle, prompt)` runs the same round on the handle's frozen binding:

````lua
local greeting = models.infer(prose)
local second_opinion = models.infer(models.get('analyst'), prose)
````

A handle from `models.get` can run `infer` even when the section has no model selection at all, while the handle-less `models.infer` fails in that case, naming the section. Neither form advertises tools or touches `sys`.

Two edge cases are worth knowing. An infer round that hits the model's length limit is reported as truncated while still returning the text produced so far. And if the model answers an infer round with tool calls, the run fails, because no tools were advertised.

## The model loop

For anything beyond one round - tool use, a retained conversation, a model that drives - `models.loop(messages, compactor?)` runs the Rust-backed model-tool loop over a message list you own:

````lua
local msgs = messages.new()
msgs:system('You are a careful researcher.')
msgs:user(prose)
models.loop(msgs)
local answer = msgs[#msgs].content
````

The loop reads the section's current model selection and tool scope at call time. With no tools in scope it performs exactly one model call. When the model emits structured tool calls, the runtime dispatches them, appends each assistant message and correlated tool result to your list, and continues until the model answers with terminal text, which is appended as the final record. The loop returns nil; the list is the result, and you may remove the terminal record explicitly when you do not want it. The optional leading-handle form `models.loop(handle, messages, compactor?)` runs the loop on the handle's frozen binding at any point in the section.

When a request would overflow the model's context window, the loop invokes the compactor you pass, or `compactors.fail` when you pass nothing. The `compactors.fail` policy always raises a typed context-exhausted error; replacement strategies are a later addition to the language.

## Building message lists

The `messages.new()` builder returns an ordinary numeric array of message records with chainable `system`, `user`, `assistant`, `tool`, and `append` methods, so `msgs:user('hi')` appends `{ role = 'user', content = 'hi' }` and returns the same list. The array stays plain data: you can write records by hand, mix both styles, and pass either to `models.loop`.

## Reading which model answered

The field `sys.model` is not readable from Lua before the section's first model or tool dispatch; reading it earlier fails with an unknown-field error. After that first dispatch it reads the catalog model id, not the alias, and `{{ sys.model }}` in later prose renders the same id.

## Environment variables

A run that needs an environment variable that is not set fails with an error naming the missing variable. A variable that is set but holds a non-Unicode value is a distinct failure.
