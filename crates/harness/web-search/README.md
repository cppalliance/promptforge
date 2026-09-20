# harness-web-search

[![Crates.io](https://img.shields.io/crates/v/harness-web-search.svg)](https://crates.io/crates/harness-web-search)
[![docs.rs](https://img.shields.io/docsrs/harness-web-search)](https://docs.rs/harness-web-search)
[![License](https://img.shields.io/crates/l/harness-web-search)](LICENSE)

A web-search tool for language models. It POSTs the model's query to the PromptForge gateway's `/tools/web_search` endpoint with a shared bearer token, so the vendor search credential never leaves the server. Arguments are validated and bounded before any network I/O, every request carries a fixed deadline, response bodies are capped and rejected on overflow, and the token is redacted from all diagnostics.

## Usage

```toml
[dependencies]
harness-web-search = "0.1"
```

```rust
use harness_web_search::WebSearch;
use harness_capabilities::Tool;

let tool = WebSearch::new("https://gateway.example.com/v1", "bearer-token")?;
let output = tool.call(serde_json::json!({ "query": "rust async runtime" })).await?;
println!("{}", output.text());
```

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
