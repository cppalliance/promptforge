# promptforge-engine

A Rust library that turns Markdown files into executable AI prompt pipelines. You write a prompt as a document - YAML frontmatter for metadata, embedded Lua for logic, prose blocks for model instructions - and the library parses it into a validated representation, then runs it as a deterministic state machine: every model round, tool call, input wait, store operation, and timer is an effect value the host performs and answers, and every boundary is an event value the host logs. Structured multi-section prompts with tool dispatch, model orchestration, concurrent fanout, and a virtual filesystem, driven by a `step`/`resume` loop the host owns.

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
