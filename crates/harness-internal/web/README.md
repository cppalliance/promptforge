# harness-web

The first-party `promptforge/web` capability: one activation unit
contributing the `promptforge/web/fetch` and `promptforge/web/search`
tools. A research prompt wants both or neither, so a prompt declares one
frontmatter line (`capabilities: [promptforge/web]`) and gets the pair.
The harness registers it in its capability registry; the engine never
names this crate. Private to the harness family in
`crates/harness-internal/`; clients reach it through the `harness`
facade. Like every harness crate, it may depend only on `promptforge`
and its container siblings.

```rust
use harness_capabilities::Capability;
use harness_web::Web;

let capability = Web::new("https://gateway.example.com/v1", "bearer-token")?;
assert_eq!(capability.id().to_string(), "promptforge/web");
# Ok::<(), promptforge::tools::ToolError>(())
```

The fetch tool enforces the crate's SSRF policy (see `harness-webfetch`);
the search tool proxies through the gateway so the vendor credential never
leaves the server (see `harness-web-search`). The host supplies the
gateway API root and bearer token when it builds the capability at
registration; the prompt never sees them.
