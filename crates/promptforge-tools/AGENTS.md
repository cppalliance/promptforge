# promptforge-tools

This crate contains runtime-agnostic tool vocabulary only: the `Tool` trait, `ToolCatalog`, `ToolId`, tool inputs and outputs, and contract errors.

- This crate never depends on HTTP clients, concrete tool providers, Lua, the parser, the executor, the gateway, or `promptforge-core`. Its dependency list is limited to vocabulary support (`async-trait`, `serde_json`, `thiserror`).
- Concrete tool implementations (`WebFetch`, `WebSearch`, future addon-host adapters) live in their own crates and depend on this one.
