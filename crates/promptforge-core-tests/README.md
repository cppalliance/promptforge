# `promptforge-core-tests`

This unpublished workspace binary crate owns complete, author-shaped PromptForge files that exercise the public `promptforge-core` parser API. It complements the core crate's narrow inline grammar tests without creating a second parser or fixture discovery mechanism.

Run the offline fixture harness with:

```text
cargo test -p promptforge-core-tests
```

Every fixture is registered by name with an explicit `include_str!` in `src/suite.rs`, so adding a file without adding its expected public structure or error contract cannot silently expand the suite. Valid fixtures assert parsed frontmatter, titles, shared Lua, section trees, and phase boundaries. Invalid fixtures assert the public error variant and a stable message fragment.

Ordinary tests do not download artifacts, start processes, contact a network, or require credentials. The binary entry point is intentionally inert until the explicit real-model runner is added.
