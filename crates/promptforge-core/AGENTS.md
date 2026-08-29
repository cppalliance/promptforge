# promptforge-core

Core owns parsing and execution: the prompt parser, the section executor, the
Lua runtime, the model catalog, and the run machinery.

## Rules

- Tool contracts and concrete tool providers remain outside this crate. The
  runtime-agnostic vocabulary (`Tool`, `ToolCatalog`, `ToolId`, tool outputs
  and contract errors) lives in `promptforge-tools`; concrete providers live
  in their own crates.
- Compatibility re-exports under `promptforge_core::tools` are allowed so
  existing `promptforge_core::tools::*` paths keep working; they re-export
  the contract crate verbatim and must not grow new vocabulary. The concrete
  `WebSearch` provider is re-exported from `promptforge-web-search` under its
  historical path; this crate must not reacquire provider code.
- The gateway model client (`GatewayClient`, the wire types, the model
  catalog and binding vocabulary) lives in `promptforge-gateway-client`.
  Compatibility re-exports under `promptforge_core::client` and
  `promptforge_core::model` follow the `tools` precedent: verbatim
  re-exports only, no new vocabulary.
- The run-scoped virtual filesystem (`Store`, `StoreRef`, the backends, and
  the error vocabulary) lives in `promptforge-store`. The compatibility
  re-export under `promptforge_core::store` follows the `tools` precedent:
  verbatim re-exports only, no new vocabulary; `WriteScope` stays
  `pub(crate)`.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
