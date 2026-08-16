# Execution

A prompt file is a Markdown document with YAML frontmatter that promptforge compiles into an executable pipeline. The frontmatter declares identity (`name`, `description`) and a version tag (`promptforge: 2`). Below the frontmatter, an H1 title heads the prompt and zero or more H2 sections supply the instructions, Lua logic, and prose that the runtime walks at execution time.

## The Run Function

Execution is a free function call over caller-owned resources. There is no process-global state. The caller owns the prompt, the execution id, the tool picker, the model catalog, the store, and the observer.

```rust
use promptforge_core::{run, Prompt, RunConfig, StoreRef, ResolutionContext};

let prompt = Prompt::parse(source, "my-execution", &observer)?;

let result = run(
    &prompt,
    "user input here",
    ResolutionContext::new(&picker, &models),
    &tools,
    &StoreRef::memory(),
    RunConfig::new("my-execution"),
).await?;
```

The run resolves the H1 block once, then walks H2 sections top to bottom. A section falls through to the next when its Lua does not return a value. An explicit return stops fall-through. When execution falls off the last section, the result is the last model reply, then the generic string "done".

## H1-Only Execution

A prompt with no H2 sections executes its H1 blocks (including any prose), and the model reply becomes the run result:

````markdown
---
name: summarize
description: Summarize the input
promptforge: 1
---

# Summarize

```lua
models.always("m", "A model suited for careful analysis")
```

Summarize this text in one paragraph.

{{ args }}
````

## Run Configuration

`RunConfig` uses a builder pattern:

```rust
RunConfig::new("execution-id")
    .observer(my_observer)
    .debug(my_debug_capture)
    .client(gateway_client)
    .cancel(cancel_handle)
    .limits(run_limits)
```

All builder methods are optional. Without `.client()`, the runtime lazily constructs one from environment variables.

## Run Limits

Configurable limits cap resource consumption:

```rust
RunLimits::new()
    .max_tool_iterations(NonZeroU32::new(24).unwrap())    // model round-trips per section
    .fanout_concurrency(NonZeroUsize::new(8).unwrap())    // parallel arms
    .max_response_bytes(NonZeroU64::new(16 * 1024 * 1024).unwrap())
    .lua_memory_bytes(NonZeroUsize::new(64 * 1024 * 1024).unwrap())
    .lua_log_events(NonZeroU32::new(1024).unwrap())
    .request_timeout(Duration::from_secs(120))
```
