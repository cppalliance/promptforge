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

-- Set it as the prompt-wide default
models.only("writer")
```

The `models.only(alias, description, opts)` form declares and designates in one atomic call. Within sections, `models.use(alias)` selects a specific model:

```lua
models.use("analyst")
```

Sections without `models.use` inherit the `models.only` default. Sections with non-empty prose but no model binding receive a clear error.

## Hard Constraints

The opts table filters the catalog before semantic resolution:

- `thinking` - boolean, required or forbidden
- `context` - minimum context window (positive integer)
- `temperature` - float in range 0.0 to 2.0
- `max_tokens` - positive integer

Duplicate model aliases or duplicate `models.only` calls are rejected atomically.

## Model Inference from Lua

`model:infer(prompt)` runs a nested model inference with tool dispatch from inside any Lua block:

```lua
local analysis = model:infer("Classify this text: " .. args)
var.classification = analysis
```

After inference, `reply` holds the model's response and `sys.reply_finish_reason` holds the finish metadata.

## Inspecting Model Properties

After binding, a model handle's frozen properties are accessible from Lua: `name`, `model_id`, `description`, `context`, `thinking`, `temperature`, `max_tokens`, and `dialect`.

## Catalog and Dialects

The library fetches a live model catalog from a gateway's `GET /v1/models` endpoint with bearer authentication. The caller provides a model catalog built from descriptors with identity, description, context window, and thinking mode (Always, Switchable, or Never).

Two tool-calling dialects ship: OpenAI (native tool calls) and Gemma-3 tool_code (emulated via content fences). Dialect resolution is automatic from model catalog evidence - endpoint capabilities, chat template markers, model id, and source provenance.
