# promptforge-api

[![Crates.io](https://img.shields.io/crates/v/promptforge-api.svg)](https://crates.io/crates/promptforge-api)
[![docs.rs](https://img.shields.io/docsrs/promptforge-api)](https://docs.rs/promptforge-api)
[![License](https://img.shields.io/crates/l/promptforge-api)](LICENSE)

A Rust library that turns Markdown files into executable AI prompt pipelines. You write a prompt as a document - YAML frontmatter for metadata, embedded Lua for logic, prose blocks for model instructions - and the library parses it into a validated representation, then executes it against any OpenAI-compatible endpoint. Structured multi-section prompts with tool dispatch, model orchestration, concurrent fanout, and a virtual filesystem, all driven from a single `run` call that returns a string.

## Usage

```toml
[dependencies]
promptforge-api = "0.1"
shared-promptforge-api = "0.1"
```

```rust
use promptforge_api::{Environment, Prompt, RunContext, RunResult};
use shared_promptforge_api::observe::NullObserver;

async fn execute(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let prompt = Prompt::parse(source, "readme", &NullObserver::default())?;
    // Capability-free agents use the default environment (no picker, empty
    // catalogs); the store handle defaults to a stock in-memory mount.
    let env = Environment::new();
    match env.run(&prompt, "", RunContext::new("readme")).await {
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
