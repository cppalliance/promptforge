# Models

Models are declared by capability description and resolved semantically against a model catalog at runtime.

## Declaring and Binding

```lua
-- Declare a model by what you need it to do
models.need("writer", "a creative writing model", {
    thinking = true,
    temperature = 0.7,
    context = 128000,
    max_tokens = 4096
})

-- Set it as the prompt-wide baseline
models.default("writer")
```

The `models.default(alias, description, opts)` form declares and designates in one atomic call; the single-alias form designates a model already declared with `models.need`. Within sections, `models.use(alias)` selects a specific model and returns its handle:

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

`handle:infer(prompt)` runs a nested model inference with tool dispatch from inside any Lua block, using that handle's specific model:

```lua
local analysis = model:infer("Classify this text: " .. args)
var.classification = analysis
```

After inference, `reply` holds the model's response and `sys.reply_finish_reason` holds the finish metadata.

`models.infer(prompt)` is the lighter path: one direct, tool-free inference round on a fresh conversation using the section's current model (the `models.use` selection, else the `models.default` baseline). It does not touch `reply` or `sys.reply_finish_reason`:

```lua
local tag = models.infer("One-word sentiment of: " .. args)
```

`models.get(alias)` returns the handle for a declared model without changing the section's model selection. Combined with `handle:infer`, it is the way to consult a different model inside a section:

```lua
local critic = models.get("critic")
local review = critic:infer("Critique this draft: " .. reply)
```

## Inspecting Model Properties

After binding, a model handle's frozen properties are accessible from Lua: `name`, `model_id`, `description`, `context`, `thinking`, `temperature`, `max_tokens`, and `dialect`.

## Catalog and Dialects

The library fetches a live model catalog from a gateway's `GET /v1/models` endpoint with bearer authentication. The caller provides a model catalog built from descriptors with identity, description, context window, and thinking mode (Always, Switchable, or Never).

Two tool-calling dialects ship: OpenAI (native tool calls) and Gemma-3 tool_code (emulated via content fences). Dialect resolution is automatic from model catalog evidence - endpoint capabilities, chat template markers, model id, and source provenance.
