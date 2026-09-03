# Models

A prompt does not name a model directly. It describes the capability it needs, and the runtime resolves that description against the catalog. This chapter teaches the three calls that declare and select models, `models.bind`, `models.default`, and `models.use`, plus the `infer` family for direct inference from Lua. Capability-based binding is what keeps a prompt portable across catalogs, so it is worth learning as a habit from the start.

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

Inside a section, `models.use('analyst')` selects a bound alias for that section. It may be called at most once per section. A section that sends prose to the model needs a model from `models.use` or from the prompt-wide default; with neither, the run fails with a model-required error.

## Inspecting a binding

The call `models.get(alias)` returns an inspectable handle with `name`, `model_id`, `description`, `context`, `thinking`, `temperature`, and `max_tokens` fields. Reading a handle does not change the model the section uses for its prose.

## Direct inference

Sometimes you want one quick model round from Lua, without prose. The call `handle:infer(prompt)` runs one tool-free inference round on the handle's frozen model, and `models.infer(prompt)` runs the same round on the section's current model. A bound alias is also callable directly as a Lua global:

````lua
local greeting = writer:infer('say hello')
````

A handle from `models.get` can run `infer` even when the section has no model binding at all, while `models.infer` fails in that case, naming the section. Neither form advertises tools, sets `reply`, or touches `sys`.

Two edge cases are worth knowing. An infer round that hits the model's length limit is reported as truncated while still returning the text produced so far. And if the model answers an infer round with tool calls, the run fails, because no tools were advertised.

## Reading which model answered

The field `sys.model` is not readable from Lua before the model scope closes; reading it in a prologue fails with an unknown-field error. After the prose runs, an epilog can read it, and `{{ sys.model }}` in later prose renders the catalog model id, not the alias.

## Environment variables

A run that needs an environment variable that is not set fails with an error naming the missing variable. A variable that is set but holds a non-Unicode value is a distinct failure.

