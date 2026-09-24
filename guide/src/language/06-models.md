# Models

A prompt does not name a model directly. It declares the roles it needs in the frontmatter, and the host binds every role to a concrete model before the run begins. This chapter teaches the declaration, the two calls that select among bound roles, `models.default` and `models.use`, plus the two operations that run model rounds from Lua, `models.infer` and `models.loop`. Declared roles are what keep a prompt portable across catalogs, so it is worth learning as a habit from the start.

## Declaring a role

Declare a model role in the frontmatter with the `models` key:

````yaml
models:
  analyst:
    keywords: [no-thinking]
    min_context: 40000
    description: careful analysis
````

Each key is a prompt-local label. A role declares a keyword set, an optional `min_context` token floor, and a description.

The keyword vocabulary is closed, and split in two. The hard keywords, `thinking` and `no-thinking`, are checked at prepare against the filled model's descriptor, as is the context minimum: a role requiring `min_context: 200000` filled with a 32k model, or requiring `thinking` filled with a model that never thinks, is reported as an unmet requirement naming the role, required versus actual. `no-thinking` is satisfied by a model that never thinks or by one whose thinking is switchable, and only a model that always thinks is refused. On a switchable model every round under the role asks for thinking off. The switch is forwarded as `chat_template_kwargs.enable_thinking`, so an upstream that ignores that field keeps its own default. The soft keywords - `frontier`, `fast`, `small`, `creative`, and `chat` - document author intent for the day a smarter fill can shop for them. An unknown keyword fails the parse; adding a keyword is a language change.

Today's fill is deliberately trivial: every declared role binds to the host's current model (in the Workshop, the dropdown's selection). The declaration is written for the full contract - roles, requirements, checks - so the same prompt runs unchanged when a smarter fill arrives; only the binding decisions change.

## The default model

The call `models.default` designates the prompt-wide default, parking a declared role by its label:

````lua
models.default("writer")
````

The label names a role declared in the frontmatter `models` key, and an unknown label is a hard error, because every label must be declared. Naming the same label again is a no-op, so a shared library replayed into every section may name the default; naming a different label fails, because the prompt-wide default cannot change mid-run.

## Selecting a model for a section

Inside a section, `models.use('analyst')` selects a bound role by its label for that section. The selection is read when a model round starts, so a later `models.use` call in the same section replaces it and steers the next round. A section that runs a model round needs a model from `models.use` or from the prompt-wide default; with neither, the call fails with a model-required error.

An optional second argument sets sampling options for the selection:

````lua
models.use('analyst', { temperature = 0, max_tokens = 1024 })
````

The table accepts two fields. `temperature` is a number from 0 to 2, written as an integer or a decimal. `max_tokens` is a positive integer that caps how many tokens the model generates. An unknown key, a value that breaks those rules, a second argument that is not a table, or a third argument fails the call with an error naming the option, required versus actual.

The options apply to the rounds that run on this selection - `models.infer(prose)`, `models.loop(msgs)`, and any round on the handle this `models.use` call returns. Rounds on the prompt-wide default or on a `models.get` handle do not see them. A later `models.use` replaces the options along with the selection, so a plain `models.use('analyst')` clears them. Leaving a field out keeps the model's default. The value passes through to the provider as given, so a provider that refuses a temperature fails the round with its own error.

## Inspecting a binding

Every bound role is also a bare global holding an inspectable handle, and `models.get(label)` returns the same handle, with `name`, `label`, `capabilities`, `model_id`, `description`, `context`, `thinking`, `temperature`, and `max_tokens` fields. On the handle `models.use` returns, `temperature` and `max_tokens` show the section's options; on every other handle, and for a field the options leave out, they read nil. Reading a handle does not change the section's selection. Handles are plain values: they have no methods, and every operation that accepts one takes it as a leading argument.

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

## Migrating from models.bind

Earlier versions bound models from Lua, resolving a prose description against the catalog at run time. The declaration moved to the frontmatter, and binding moved to prepare. Before:

````lua
models.bind('analyst', 'a careful model that does not think')
models.default('analyst')
````

After:

````yaml
models:
  analyst:
    keywords: [no-thinking]
    description: careful analysis
````

````lua
models.default('analyst')
````

The `models.bind` call is removed. What was its prose description now documents the role, the hard requirements move into `keywords` and `min_context`, and `models.default` and `models.use` name declared labels only. The old `models.bind` options `temperature` and `max_tokens` now go in the `models.use` options table, as in `models.use('analyst', { temperature = 0.2 })`.
