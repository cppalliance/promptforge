# promptforge-core

The PromptForge runtime core: the pieces that turn a prompt markdown file into a
model call.

This crate is private and unpublished (`publish = false`); it is consumed by the
other crates in the [PromptForge workspace](https://github.com/cppalliance/promptforge).

## What it does

- **`parser`** reads a prompt file's frontmatter and sections into a `Prompt`.
- **`client`** talks to an OpenAI-compatible chat completions endpoint.
- **`execute`** runs the parsed prompt: it resolves the H1 block once, then walks
  the H2 sections top to bottom (fall-through) and returns the run's result.
- **`observe`** is the report-only seam a caller uses to watch a long run in
  flight; **`debug`** is a separate opt-in seam for raw request/response capture.
- **`tools`** is the `Tool` trait the executor dispatches during a model's
  tool-call loop, plus the built-in `web_search` tool.

A source is a PromptForge prompt only when its frontmatter declares a
`promptforge:` version; `promptforge_version` reports it (or `None`), and the
runtime refuses a source that lacks a supported version.

## Public surface

Every public boundary returns its own typed error (`ParseError`, `RunError`,
`CompletionError`, `DialectError`, `tools::ToolError`, `store::StoreError`);
there is no crate-wide public `Error`/`Result`. Public error types expose a
stable `kind()` classifier, and public structs are `#[non_exhaustive]` so they
can evolve without a breaking change.

## Example

```rust
use promptforge_core::{Prompt, promptforge_version};
use promptforge_core::observe::NullObserver;

let source = "---\nname: greeter\ndescription: says hi\npromptforge: 1\n---\n\n# Greeter\n\n## Say hi\n\nSay hello.\n";

// Version detection gates whether the runtime will accept the source.
assert_eq!(promptforge_version(source), Some(1));

// Parse into a `Prompt` (returns a `Result<_, ParseError>`).
let prompt = Prompt::parse(source, "doc-example", &NullObserver::default()).unwrap();
assert_eq!(prompt.title(), "Greeter");
```

## License

Boost Software License 1.0 (BSL-1.0).
