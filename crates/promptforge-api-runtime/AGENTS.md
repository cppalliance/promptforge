# promptforge-api-runtime

This crate owns PromptForge document execution and run orchestration.

- Historical `promptforge_api_runtime` compatibility paths are verbatim re-exports from the owning crates. Do not create new compatibility vocabulary here.
- Concrete providers stay in their provider crates. Core may re-export them under a historical path but never reacquires provider implementation.
- Store write scope remains private to Core's execution machinery.
- The executor imports parser, Lua, model-client, store, tool, and host-support vocabulary from the private crates under `crates/promptforge/`. Those crates never depend on this executor.
- One door: this crate and `promptforge-api-types` are the only promptforge-* dependencies an outside crate (workshop-*, gateway-*, shared-*, build-*) may name. The crates under `crates/promptforge/` are private to the family, this crate is the only outside crate permitted to depend into the container, and `cargo test -p build-xtask` enforces the boundary.
- The input broker backs only the script-side `user_input()` function. No `user_input` tool is ever advertised to a model unless a prompt explicitly adds it.
