# promptforge-core

This crate owns PromptForge document execution and run orchestration.

- Historical `promptforge_core` compatibility paths are verbatim re-exports from the owning crates. Do not create new compatibility vocabulary here.
- Concrete providers stay in their provider crates. Core may re-export them under a historical path but never reacquires provider implementation.
- Store write scope remains private to Core's execution machinery.
- The executor imports parser, Lua, model-client, store, tool, and host-support vocabulary from their owning crates. Those crates never depend on this executor.
