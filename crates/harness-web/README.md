# harness-web

The `promptforge/web` capability a Host registers: one activation unit contributing the `promptforge/web/fetch` and `promptforge/web/search` tools. A research prompt wants both or neither, so a prompt declares one frontmatter line (`capabilities: [promptforge/web]`) and gets the pair.

The fetch tool fetches a URL and returns its main content as markdown, and it is the SSRF boundary between a model-supplied URL and the network: every hop is revalidated at DNS-resolution time, so names that resolve inward, rebinding answers, and redirect chains that point somewhere they should not are refused. The search tool validates the model's arguments and runs the search through the Host's `SearchProvider`, so a search vendor's credential stays wherever the provider keeps it.

A Host registers `Web` in its capability registry and provides two services beside it: its `SearchProvider` under `SEARCH_PROVIDER`, and the tokio runtime handle every fetch runs on under `TOKIO_RUNTIME`.

```rust
use std::sync::Arc;

use harness::capability::{CapabilityRegistry, HostServices};
use harness_web::{SEARCH_PROVIDER, TOKIO_RUNTIME, Web};

let mut capabilities = CapabilityRegistry::new();
capabilities.register(Arc::new(Web::new()))?;
let mut services = HostServices::new();
services.provide(&SEARCH_PROVIDER, Arc::new(my_provider))?;
services.provide(&TOKIO_RUNTIME, Arc::new(tokio::runtime::Handle::current()))?;
```

It sits at the `crates/` root beside `harness` and may depend only on `harness`, `promptforge`, and third-party crates.

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.
