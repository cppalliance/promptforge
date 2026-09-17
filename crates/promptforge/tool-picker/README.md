# promptforge-tool-picker

[![Crates.io](https://img.shields.io/crates/v/promptforge-tool-picker.svg)](https://crates.io/crates/promptforge-tool-picker)
[![docs.rs](https://img.shields.io/docsrs/promptforge-tool-picker)](https://docs.rs/promptforge-tool-picker)
[![License](https://img.shields.io/crates/l/promptforge-tool-picker)](LICENSE)

A sentence-embedding resolver that turns a prose description like "read a file from disk" into the tool that does it - no LLM call, no network, no guessing. You describe your tools in prose, build a picker over the catalog, and ask it which tool a need refers to. Querying is a dot product, not an API call. The model is compiled into the library, so there is no path to configure and no weights to ship.

## Usage

```toml
[dependencies]
promptforge-tool-picker = "0.1"
```

```rust
use promptforge_tool_picker::{ToolId, ToolDescriptor, Catalog, Picker};

let catalog = Catalog::new(vec![
    ToolDescriptor::new(ToolId::new("files", "read_file"), "Read a file from disk", schema),
]);
let picker = Picker::new(&catalog)?;
let decision = picker.pick("read a file")?;
```

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
