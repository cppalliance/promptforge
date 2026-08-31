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

## Binaries and features

Two products ship from this workspace: `promptforge-gateway.exe`, the lean server, and `promptforge-workshop.exe`, the batteries-included desktop app that boots the gateway in-process. The binary is the feature set; features are not product variants.

- A Cargo feature exists only to gate a real constraint: a toolchain requirement (`cuda`) or a heavy native build (`local`). Do not add features that merely describe product shape.
- `config-ui` is a default gateway feature and always present in the desktop build. The UI `dist/` artifacts are checked into the repo and verified in `build.rs`, so plain gateway builds need no Node 22; only UI development does. Keep the checked-in artifact fresh: `npm run package` in the UI directory after UI edits, before committing.
- Never make `workshop` a default gateway feature. The desktop exe is the everything-build; the gateway stays the lean one. That asymmetry is the product boundary.
- Keep `cargo check -p promptforge-gateway --no-default-features` green. Nobody ships that build, but it is the cheap gate that catches optional-feature types leaking into core paths.

## Progress

Long-running work reports progress through `promptforge-progress`: attach an operation tree to the process `ProgressHub` - or register leaves on the current operation's tree when one exists - and report through the `ProgressHandle` it returns. Never invent a parallel progress channel: no ad-hoc callbacks, stage strings, or direct status-bus calls for fractional progress. Producers never format output; renderers subscribe to the hub.

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
- Config UI: `npm run typecheck && npm run build && npm test` in `crates/promptforge-gateway-config-ui/ui` (the test suite imports the built `dist/app.js`, so build first).
