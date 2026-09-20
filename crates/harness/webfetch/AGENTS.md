# harness-webfetch

This crate fetches and converts one caller-supplied URL into Markdown.

- The caller defines URL scope. This provider does not search, crawl, or discover targets.
- Every initial request and redirect hop uses the guarded resolver, address pinning, redirect policy, and bounded body handling. No hop may bypass SSRF validation.
- The `Tool` trait comes from `harness-capabilities`; tool id, schema, output, and error vocabulary from `promptforge-api-types`. This provider never depends on Core or a Gateway product crate.
- Family rules: a harness crate, private to `crates/harness/`; depends on `harness-capabilities`, `promptforge-api-types`, and container siblings only. Tests spawn their mock servers through `harness-runner`'s instrumented wrapper, never `tokio::spawn`.
