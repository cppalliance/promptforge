# PromptForge

Multi-crate Rust workspace for the PromptForge pipeline runtime.

## Do more with less

This rule outranks every other rule here. Before you add a frontmatter field, a configuration key, a public type, or a new resolution path, answer one question: can this be built with what is already there? Lua already runs, sandboxed and budgeted. The run-scoped store already exists, has a file backend, and is already reachable from Lua. The catalog already resolves globs and exceptions. New machinery has to beat all of that on the merits, and "it would be tidier" is not a merit. If you add it anyway, say in the commit which existing facility you considered and why it could not carry the work.

## Rules

- After completing work (compiles + tests pass), update README.md if the public surface changed.
- Every public type, function, and module must have a `///` doc comment. `cargo doc` is the project documentation.
- Each crate's own AGENTS.md binds its subtree. Read the ones governing the paths you touch before writing or reviewing code.
- The existing test suite stays green and intact during refactors: fix forward; never rewrite a test to make it pass.
- Library and serve paths never call `process::exit` or install process-global state; failures return through the spawn handshake.
- Runtime and serve paths never compile native dependencies or invoke compilers or build tools. Native compilation belongs to the Cargo build or packaging process; runtime may only verify, stage, and launch build-produced artifacts.
- Do NOT look at files outside this repo for reference.
- The plan is the spec. Work from the plan and AGENTS.md only.

## Comments

`///` doc comments on public items are mandatory (above) and are not what this section governs. Any other comment earns its place by exactly four things: a non-obvious why, an invisible constraint no type or test enforces, an external-bug workaround, or a subtle ordering requirement. Comments that narrate what the code already says are deleted on sight. A module doc earns its place by documenting the domain, not by restating the file name.

Every platform or external-bug workaround carries its upstream issue URL inline, in the comment that explains it. When the workaround dies, the URL says when it can be buried.

```rust
// wry's drag-drop handler suppresses HTML5 drag events on Windows
// (https://github.com/tauri-apps/tauri/issues/15138), so ...
```

## Verify

- Rust: `cargo test` at the workspace root.
- UI: `npm run typecheck && npm test` in `crates/promptforge-workshop-server/ui`.
