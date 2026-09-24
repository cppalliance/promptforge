# crates/promptforge-internal/

`crates/promptforge-internal/` is the promptforge family's private container - outside crates reach the family only through the `promptforge` facade at `crates/promptforge/`, which is the one crate permitted to depend into this container.

## promptforge-engine

The PromptForge engine: the sans-IO `Run` state machine that executes a parsed prompt's sections as effects a host performs and answers. The facade re-exports its run, effect, context, and error types. Depends on promptforge-types, promptforge-lua, promptforge-parser, promptforge-store, promptforge-vfs, and promptforge-model-client.

## promptforge-types

The shared vocabulary most of the facade is built on: the tool, capability, and global-name vocabulary, the model identity, catalog, and streaming wire vocabulary, chain and task identity with replay provenance and flags, and the run timestamp. It also holds the host-support primitives: untrusted-content guards, cooperative cancellation, run observation, and the metrics vocabulary. Nearly every engine crate builds on it and reports through it. No workspace dependencies beyond workspace-hack and the doctest-only `promptforge` dev-dependency the root `AGENTS.md` excepts, which its doc examples alone compile against.

## promptforge-lua

The sandboxed Lua runtime: the section VM, the coroutine protocol, and the host surface over a restricted mlua VM. The engine executes every section's Lua block through it, and the parser splits fences for it. Depends on promptforge-types, promptforge-model-client, and promptforge-store.

## promptforge-parser

The prompt document parser: YAML frontmatter, the H1 and nested-section tree, and exact lua fence splitting. It is the engine's first stop for every prompt file. Depends on promptforge-types and promptforge-lua.

## promptforge-store

Run-scoped virtual files: the Store facade over the VFS, built by `Store::new(&access)`. Lua sections and the model share run files through it. Depends on promptforge-vfs.

## promptforge-vfs

The PromptForge virtual filesystem: canonical interned paths, the claims model, the mount router, and the host and memory backends, plus the `/_promptforge` mount layout, the stock empty handle, and the mode policy. It is the permanent bottom of the dependency stack. Std only - no dependencies at all, enforced by its own manifest test.

## promptforge-model-client

The model vocabulary: the chat-completions wire types and SSE reassembly a `Chat` effect exchanges, and the model catalog and binding vocabulary. The engine and the Lua host call models through it. Depends on promptforge-types; it owns no transport - the HTTP client is the harness's (`harness-models`).
