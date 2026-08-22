# promptforge-core

[![Crates.io](https://img.shields.io/crates/v/promptforge-core.svg)](https://crates.io/crates/promptforge-core)
[![docs.rs](https://img.shields.io/docsrs/promptforge-core)](https://docs.rs/promptforge-core)
[![License](https://img.shields.io/crates/l/promptforge-core)](LICENSE)

A Rust library that turns Markdown files into executable AI prompt pipelines. You write a prompt as a document - YAML frontmatter for metadata, embedded Lua for logic, prose blocks for model instructions - and the library parses it into a validated representation, then executes it against any OpenAI-compatible endpoint. Structured multi-section prompts with tool dispatch, model orchestration, concurrent fanout, and a virtual filesystem, all driven from a single `run` call that returns a string.

## Usage

```toml
[dependencies]
promptforge-core = "0.1"
promptforge-tool-picker = "0.1"
```

```rust
use promptforge_core::model::ModelCatalog;
use promptforge_core::observe::NullObserver;
use promptforge_core::store::StoreRef;
use promptforge_core::tools::ToolCatalog;
use promptforge_core::{Prompt, ResolutionContext, RunConfig, run};
use promptforge_tool_picker::{Catalog, Config, ToolPicker};

async fn execute(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let prompt = Prompt::parse(source, "readme", &NullObserver::default())?;
    let picker = ToolPicker::build(Catalog::new(Vec::new()), Config::default())?;
    let models = ModelCatalog::empty();
    let tools = ToolCatalog::new(&[])?;
    let store = StoreRef::memory();

    let result = run(
        &prompt,
        "",
        ResolutionContext::new(&picker, &models, &tools),
        &store,
        RunConfig::new("readme"),
    )
    .await?;
    Ok(result)
}
```

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
