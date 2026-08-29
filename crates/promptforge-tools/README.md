# promptforge-tools

The runtime-agnostic tool contract for the PromptForge pipeline: the [`Tool`]
trait an executable tool implements, the validated [`ToolCatalog`] a harness
builds once and shares across runs, stable [`ToolId`] identity, and the
trust-carrying [`ToolOutput`] / model-safe [`ToolError`] vocabulary.

This crate holds vocabulary only. Concrete tools (web fetch, web search),
the prompt parser, and the executor live in their own crates and depend on
this one.

```rust
use promptforge_tools::{ToolCatalog, ToolId};

let catalog = ToolCatalog::new(&[])?;
let missing = ToolId::new("promptforge", "web_fetch")?;
assert!(catalog.get(&missing).is_none());
```

License: BSL-1.0
