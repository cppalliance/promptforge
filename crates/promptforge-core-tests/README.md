# `promptforge-core-tests`

This unpublished workspace binary crate owns complete, author-shaped PromptForge files that exercise the public `promptforge-core` parse, deterministic declaration-binding, and offline execution APIs. It complements the core crate's narrow inline grammar and lifecycle tests without creating a second parser or fixture discovery mechanism.

Run the offline fixture harness with:

```text
cargo test -p promptforge-core-tests
```

Every fixture is registered by name with an explicit `include_str!` in `src/suite.rs`, so adding a file without adding its expected public structure, error contract, or execution assertion cannot silently expand the suite. Valid fixtures assert parsed frontmatter, titles, shared Lua, section trees, and phase boundaries. Invalid fixtures assert the public error variant and a stable message fragment. Tool-free execution fixtures run shared declarations through public `bind_tool_declarations` with a deterministic no-tools resolver, then use the public parsed-`Prompt` compatibility input to `execute::run`. They assert exact Lua checkpoint sequences, scalar preamble early return, and store-backed fall-through across isolated sections using stable execution IDs and a mutex-backed observer. A concurrent regression partitions one shared recording by execution ID.

The shipped-prompt smoke test discovers every Markdown file under the repository's `prompts/` directory, rejects concrete tool names, and requires each file to parse. The MCP server's owner tests retain the shipped-prompt semantic binding assertion against its complete live registry.

The harness does not construct a picker, load a model, provision production tools, start processes, send network requests, make gateway or generation-model calls, or require credentials. Its only direct workspace dependency is `promptforge-core`; a cold workspace build may still acquire assets required by that crate's existing transitive picker dependency. The binary entry point is intentionally inert until the explicit real-model runner is added.
