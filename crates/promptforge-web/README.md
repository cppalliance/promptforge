# promptforge-web

The first-party `promptforge/web` capability: one activation unit
contributing the `promptforge/web/fetch` and `promptforge/web/search`
tools. A research prompt wants both or neither, so a prompt declares one
frontmatter line (`capabilities: [promptforge/web]`) and gets the pair.

```rust
use promptforge_web::Web;
use shared_promptforge_api::capabilities::Capability;

let capability = Web::new("https://gateway.example.com/v1", "bearer-token")?;
assert_eq!(capability.id().to_string(), "promptforge/web");
# Ok::<(), shared_promptforge_api::tools::ToolError>(())
```

The fetch tool enforces the crate's SSRF policy (see `promptforge-webfetch`);
the search tool proxies through the gateway so the vendor credential never
leaves the server (see `promptforge-web-search`). The host supplies the
gateway API root and bearer token when it builds the capability at
registration; the prompt never sees them.
