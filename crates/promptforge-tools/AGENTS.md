# promptforge-tools

This crate contains runtime-agnostic tool vocabulary only: the `Tool` trait,
`ToolCatalog`, `ToolId`, tool inputs and outputs, and contract errors.

## Rules

- This crate never depends on HTTP clients, concrete tool providers, Lua, the
  parser, the executor, the gateway, or `promptforge-core`. Its dependency
  list is limited to vocabulary support (`async-trait`, `serde_json`,
  `thiserror`).
- Concrete tool implementations (`WebFetch`, `WebSearch`, future addon-host
  adapters) live in their own crates and depend on this one.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
