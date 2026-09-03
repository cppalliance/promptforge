# promptforge-core

Core owns parsing and execution: the prompt parser, the section executor, the Lua runtime, the model catalog, and the run machinery.

- Compatibility re-exports under historical `promptforge_core::*` paths follow one precedent: verbatim re-exports of the owning crate only, no new vocabulary grown here. That covers `tools` (`promptforge-tools`; concrete `WebSearch` from `promptforge-web-search` under its historical path - never reacquire provider code), `client`/`model` (`promptforge-model-client`), `store` (`promptforge-store`; `WriteScope` stays `pub(crate)`), `lua`/`observe`/crate-root `CancelHandle` (`promptforge-lua` and `promptforge-core-support`), and `parser` (`promptforge-parser`).
- Vocabulary ownership stays outside this crate's body: tool contracts and providers in `promptforge-tools` and provider crates; gateway model client in `promptforge-model-client`; run-scoped store in `promptforge-store`; Lua sandbox and host surface in `promptforge-lua` with host-support primitives in `promptforge-core-support`; prompt document parser in `promptforge-parser`. The executor imports from `promptforge-lua` and `promptforge-parser`, never the reverse.
