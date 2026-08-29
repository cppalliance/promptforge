# promptforge-store

This crate is the run-scoped virtual filesystem: the `Store` backend contract,
the `MemStore` and `FileStore` backends, path and glob validation, the shared
`StoreRef` handle, and the fanout write-scope registry.

## Rules

- Virtual filesystem only. No executor, Lua, or tool dependencies: the crate
  never imports `promptforge-core` subsystems (parser, `mlua`, execute,
  observe) or `promptforge-tools`; consumers adapt to this crate, never the
  reverse.
- The `#[doc(hidden)]` items (`WriteScope`, `StoreRef::next_write_token`,
  `StoreRef::write_scoped`, `StoreError::not_found`,
  `StoreError::invalid_range`) are cross-crate seams for `promptforge-core`'s
  fanout machinery and test doubles, not host API; they must not gain
  documented status without a design change.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
