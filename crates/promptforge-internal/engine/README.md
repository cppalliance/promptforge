# promptforge-engine

A Rust library that turns Markdown files into executable AI prompt pipelines. You write a prompt as a document - YAML frontmatter for metadata, embedded Lua for logic, prose blocks for model instructions - and the library parses it into a validated representation, then runs it as a deterministic state machine: every model round (`Chat`), tool call (`ToolCall`), store operation (`Store`), timer (`Timer`), and read of a task's reported history (`TaskEvents`) is an effect value the Harness performs and answers, and every boundary is an event value the Harness logs. Structured multi-section prompts with tool dispatch, model orchestration, concurrent fanout, and a virtual filesystem, driven by a `step`/`resume` loop the Harness owns.

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## License

Licensed under the Boost Software License 1.0.
