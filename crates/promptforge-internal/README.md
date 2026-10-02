# crates/promptforge-internal/

`crates/promptforge-internal/` is the promptforge family's private container - outside crates reach the family only through the `promptforge` facade at `crates/promptforge/`, which is the one crate permitted to depend into this container.

## promptforge-engine

The Engine's executor: the sans-IO `Run` state machine that executes a parsed prompt's sections as effects the Harness performs and answers. The facade re-exports its run, effect, context, and error types. Depends on promptforge-types, promptforge-lua, promptforge-parser, promptforge-vfs, and promptforge-model-client.

## promptforge-types

The shared vocabulary most of the facade is built on: the tool, capability, and global-name vocabulary, the model identity, catalog, and streaming wire vocabulary, chain and task identity with replay provenance and flags, and the run timestamp. It also holds the run-support primitives: untrusted-content guards, cooperative cancellation, run observation, and the metrics vocabulary. Nearly every Engine crate builds on it and reports through it. No workspace dependencies beyond workspace-hack and the doctest-only `promptforge` dev-dependency the root `AGENTS.md` excepts, which its doc examples alone compile against.

## promptforge-lua

The sandboxed Lua runtime: the section VM, the coroutine protocol, and the Engine globals over a restricted mlua VM. `promptforge-engine` executes every section's Lua block through it, and the parser splits fences for it. Depends on promptforge-types, promptforge-model-client, and promptforge-vfs.

## promptforge-parser

The prompt document parser: YAML frontmatter, the H1 and nested-section tree, and exact lua fence splitting. It is the Engine's first stop for every prompt file. Depends on promptforge-types and promptforge-lua.

## promptforge-vfs

The PromptForge virtual filesystem: canonical interned paths, the claims model, the mount router, and the real-filesystem and memory backends, and the mode policy. It is the permanent bottom of the dependency stack. Std only - no dependencies at all, enforced by its own manifest test.

## promptforge-model-client

The model vocabulary: the chat-completions wire types and SSE reassembly a `Chat` effect exchanges, and the model catalog and binding vocabulary. The model catalog types (`ModelCatalog`, `ModelDescriptor`, `ModelId`, `ThinkingMode`) are defined in promptforge-types and re-exported by model-client's `model` module. `promptforge-engine` and the Engine globals in `promptforge-lua` call models through it. Depends on promptforge-types; the HTTP client that carries each round is the Harness's (`harness-models`).
