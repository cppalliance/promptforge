# Models

Models are declared by capability description and resolved semantically against a model catalog at runtime.

## Declaring and Binding

```lua
-- Declare a model by what you need it to do
models.bind("writer", "a creative writing model", {
    thinking = true,
    temperature = 0.7,
    context = 128000,
    max_tokens = 4096
})

-- Set it as the prompt-wide baseline
models.default("writer")
```

The `models.default(alias, description, opts)` form declares and designates in one atomic call; the single-alias form designates a model already declared with `models.bind`. Within sections, `models.use(alias)` selects a specific model and returns its handle:

```lua
local analyst = models.use("analyst")
```

Sections without `models.use` inherit the `models.default` baseline. A prompt can carry both - the baseline applies everywhere a section does not override it. Sections with non-empty prose but no model binding receive a clear error.

## Hard Constraints

The opts table filters the catalog before semantic resolution:

- `thinking` - boolean, required or forbidden
- `context` - minimum context window (positive integer)
- `temperature` - float in range 0.0 to 2.0
- `max_tokens` - positive integer

Duplicate model aliases or duplicate `models.default` calls are rejected atomically. `models.use` may be called at most once per section.

## Model Inference from Lua

`infer` has one shape: a single tool-free inference round on a fresh conversation. It never sets `reply` and never touches `sys`. Two forms exist:

```lua
-- The section's current model (the models.use selection, else the models.default baseline)
local tag = models.infer("One-word sentiment of: " .. args)

-- Any declared model, via its handle
local critic = models.get("critic")
local review = critic:infer("Critique this draft: " .. reply)
```

`models.get(alias)` returns the handle for a declared model without changing the section's model selection, so `handle:infer` is the way to consult a different model inside a section. A Lua block that needs tools uses `execute` on a section instead.

## Inspecting Model Properties

After binding, a model handle's frozen properties are accessible from Lua: `name`, `model_id`, `description`, `context`, `thinking`, `temperature`, and `max_tokens`.

## Model Catalog

The library fetches a live model catalog from a gateway's `GET /v1/models` endpoint with bearer authentication. The caller provides a model catalog built from descriptors with identity, description, context window, and thinking mode (Always, Switchable, or Never).
