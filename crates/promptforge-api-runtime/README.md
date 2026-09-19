# promptforge-api-runtime

[![Crates.io](https://img.shields.io/crates/v/promptforge-api-runtime.svg)](https://crates.io/crates/promptforge-api-runtime)
[![docs.rs](https://img.shields.io/docsrs/promptforge-api-runtime)](https://docs.rs/promptforge-api-runtime)
[![License](https://img.shields.io/crates/l/promptforge-api-runtime)](LICENSE)

A Rust library that turns Markdown files into executable AI prompt pipelines. You write a prompt as a document - YAML frontmatter for metadata, embedded Lua for logic, prose blocks for model instructions - and the library parses it into a validated representation, then executes it against any OpenAI-compatible endpoint. Structured multi-section prompts with tool dispatch, model orchestration, concurrent fanout, and a virtual filesystem, all driven from a single `run` call that returns a string.

## Usage

```toml
[dependencies]
promptforge-api-runtime = "0.1"
```

```rust
use promptforge_api_runtime::types::observe::NullObserver;
use promptforge_api_runtime::types::timestamp::Timestamp;
use promptforge_api_runtime::{Environment, Prompt, RunContext, RunResult};

async fn execute(source: &str, seed: u64, started_at: Timestamp) -> Result<String, Box<dyn std::error::Error>> {
    let prompt = Prompt::parse(source, "readme", &NullObserver::default())?;
    // Capability-free agents use the default environment (no registry, empty
    // catalogs); the store handle defaults to a stock in-memory mount. The
    // host draws the seed (from a CSPRNG) and stamps the start instant: the
    // engine reads neither the OS RNG nor the clock.
    let env = Environment::new();
    match env.run(&prompt, "", RunContext::new("readme", seed, started_at)).await {
        RunResult::Ok(text) => Ok(text),
        RunResult::Cancelled => Err("the run was cancelled".into()),
        RunResult::Failure(error) => Err(error.into()),
    }
}
```

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
