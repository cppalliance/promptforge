# promptforge-api-runtime

A Rust library that turns Markdown files into executable AI prompt pipelines. You write a prompt as a document - YAML frontmatter for metadata, embedded Lua for logic, prose blocks for model instructions - and the library parses it into a validated representation, then runs it as a deterministic state machine: every model round, tool call, input wait, store operation, and timer is an effect value the host performs and answers, and every boundary is an event value the host logs. Structured multi-section prompts with tool dispatch, model orchestration, concurrent fanout, and a virtual filesystem, driven by a `step`/`resume` loop the host owns.

## Usage

```toml
[dependencies]
promptforge-api-runtime.workspace = true
```

```rust
use std::sync::Arc;

use promptforge_api_runtime::types::timestamp::Timestamp;
use promptforge_api_runtime::{EffectAnswer, Environment, Prompt, Run, RunContext, RunResult, Step};

fn execute(source: &str, seed: u64, started_at: Timestamp) -> Result<String, Box<dyn std::error::Error>> {
    // A parse returns its parse-time events beside the outcome, for the
    // host to log; the engine never reads them back.
    let (prompt, _parse_events) = Prompt::parse(source, "readme");
    let prompt = prompt?;
    // Capability-free agents use the default environment (an empty tool
    // catalog); the store handle defaults to a stock in-memory mount. The
    // host draws the seed (from a CSPRNG) and stamps the start instant: the
    // engine reads neither the OS RNG nor the clock.
    let env = Environment::new();
    let (ctx, requirements) = env.prepare(&prompt, RunContext::new("readme", seed, started_at));
    if let Some(refusal) = requirements.refusal() {
        return Err(refusal.into());
    }
    let mut run = Run::new(Arc::new(prompt), "", ctx);
    loop {
        match run.step() {
            Step::Done { result: RunResult::Ok(text), .. } => return Ok(text),
            Step::Done { result: RunResult::Cancelled, .. } => return Err("the run was cancelled".into()),
            Step::Done { result: RunResult::Failure(error), .. } => return Err(error.into()),
            Step::Pending { effects, events } => {
                // Log `events`; perform each effect (a model round, a tool
                // call, an input wait, a store operation, a timer) however
                // the host likes and answer it. This host performs nothing.
                let _ = events;
                for (id, _provenance, _effect) in effects {
                    run.resume(id, EffectAnswer::Dropped);
                }
            }
        }
    }
}
```

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
