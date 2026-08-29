# promptforge-webfetch

[![Crates.io](https://img.shields.io/crates/v/promptforge-webfetch.svg)](https://crates.io/crates/promptforge-webfetch)
[![docs.rs](https://img.shields.io/docsrs/promptforge-webfetch)](https://docs.rs/promptforge-webfetch)
[![License](https://img.shields.io/crates/l/promptforge-webfetch)](LICENSE)

A web-fetching tool for language models. Hand it a URL and it fetches the page, extracts the useful content, and returns it as markdown the model can cite - while enforcing an SSRF boundary that prevents the model from reaching your internal network no matter what URL it supplies. The security is layered and runs at DNS-resolution time on every hop, catching names that resolve inward, rebinding attacks, and redirect chains that point somewhere they should not.

## Usage

```toml
[dependencies]
promptforge-webfetch = "0.1"
```

```rust
use promptforge_webfetch::WebFetch;
use promptforge_tools::Tool;

let tool = WebFetch::new();
let output = tool.call(serde_json::json!({ "url": "https://example.com" })).await?;
println!("{}", output.text());
```

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
