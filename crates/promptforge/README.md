# promptforge

Integrator-facing facade for the PromptForge library product. Two entry points, no logic of its own.

## Entry points

```rust
// Document prompts (.md): sections, prose, the built-in tool loop.
use promptforge::pipeline::{run, RunConfig, RunError};

// Agent programs (.lua): the Lua program owns the loop.
use promptforge::agent::{run, AgentConfig, AgentError};
```

Substrate types (parser, store, tools, models) come from their own crates - this package depends only on `promptforge-core` and `promptforge-agent`.
