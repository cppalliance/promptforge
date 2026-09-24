# promptforge-api-runtime

This crate owns PromptForge document execution and run orchestration. The crate root is the one public facade: hosts name the run, effect, context, and error types through `promptforge_api_runtime`, `execute` is private, and only `model`, `parser`, `input`, and `types` remain public vocabulary modules. The single-public-crate rule that makes it, with `promptforge-api-types`, the only promptforge-* dependency an outside crate may name is stated in the root `AGENTS.md` and enforced by `cargo test -p build-xtask`.

- The crate root is the only public path for host types. The thin `lua`, `untrusted`, `store`, and `tools` modules are crate-internal import surfaces for the crates that own them; do not add public compatibility paths or re-exports.
- Concrete providers stay in their provider crates. `promptforge-api-runtime` may re-export them for crate-internal use but never reacquires provider implementation.
- Store access is decided only by the executor: every `Access` handle is minted from the chain's claims inside the engine; a host performing a `Store` effect uses the handle it was given and never derives, widens, or retains store scope.
- The executor imports parser, Lua, model-client, store, tool, and host-support vocabulary from the private crates under `crates/promptforge/`. Those crates never depend on this executor.
