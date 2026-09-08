# PromptForge

Multi-crate Rust workspace for the PromptForge pipeline runtime, inference gateway, and Workshop desktop product.

- Prefer types and compiler checks, then behavior tests and deterministic fault injection. Add a structural check only with explicit user approval for a stable product or security boundary that has no ordinary equivalent.
- Repository policy binds plans. A plan cannot introduce a source parser, snapshot, allowlist, count, ceiling, topology check, import walker, or other structural enforcement unless the user explicitly approves that exception.
- Behavior changes ship with tests in the same change. Preserve product and behavior tests during refactors. Structural tests that an approved plan identifies as unsupported may be removed without replacement by another structural proxy.
- Keep four cross-product dependency rules: Gateway product crates cannot depend on Workshop product crates; PromptForge product crates cannot depend on Gateway or Workshop product crates; Gateway product crates cannot depend on PromptForge product crates; Workshop product crates cannot depend on Gateway product crates.
- Do more with less. Before adding a frontmatter field, configuration key, public type, or resolution path, determine whether sandboxed Lua, the run-scoped store, or the catalog already carries the work. New machinery must have a material benefit beyond tidiness.
- A Cargo feature gates a real constraint such as a toolchain requirement or heavy native build. It does not describe product shape. Feature-disabled builds must not leak optional types into core paths.
- Runtime and serve paths never compile native dependencies or invoke build tools. Library and serve paths return failures instead of exiting the process or installing process-global state.
- Long-running work reports through `shared-progress`. Producers report operation state, hosts forward it, and renderers format it.
- Unsafe code stays in its explicitly owned boundary. Every unsafe block documents its safety invariants immediately before the block.
- Comments explain a non-obvious constraint, ordering requirement, or workaround. Every platform or external-bug workaround cites its upstream issue URL in the explanatory comment.
