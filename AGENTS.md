# PromptForge

Multi-crate Rust workspace for the PromptForge pipeline runtime.

## Do more with less

This rule outranks every other rule here. Before you add a frontmatter field, a configuration key, a public type, or a new resolution path, answer one question: can this be built with what is already there? Lua already runs, sandboxed and budgeted. The run-scoped store already exists, has a file backend, and is already reachable from Lua. The catalog already resolves globs and exceptions. New machinery has to beat all of that on the merits, and "it would be tidier" is not a merit. If you add it anyway, say in the commit which existing facility you considered and why it could not carry the work.

## Rules

- After completing work (compiles + tests pass), update README.md if the public surface changed.
- Every public type, function, and module must have a `///` doc comment. `cargo doc` is the project documentation.
- STYLE.md at the repo root carries the comment, workaround, error-zone, layering, and test conventions for the workshop crates.
- Do NOT look at files outside this repo for reference.
- The plan is the spec. Work from the plan and AGENTS.md only.
