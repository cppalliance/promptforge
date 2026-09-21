# promptforge-api-runtime

This crate owns PromptForge document execution and run orchestration. The single-public-crate rule that makes it, with `promptforge-api-types`, the only promptforge-* dependency an outside crate may name is stated in the root `AGENTS.md` and enforced by `cargo test -p build-xtask`.

- Historical `promptforge_api_runtime` compatibility paths are verbatim re-exports from the owning crates. Do not create new compatibility vocabulary here.
- Concrete providers stay in their provider crates. `promptforge-api-runtime` may re-export them under a historical path but never reacquires provider implementation.
- Store access is decided only by the executor: every `Access` handle is minted from the chain's claims inside the engine; a host performing a `Store` effect uses the handle it was given and never derives, widens, or retains store scope.
- The executor imports parser, Lua, model-client, store, tool, and host-support vocabulary from the private crates under `crates/promptforge/`. Those crates never depend on this executor.
