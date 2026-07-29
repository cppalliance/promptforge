# PromptForge

Multi-crate Rust workspace for the PromptForge pipeline runtime.

## Rules

- After completing work (compiles + tests pass), update STATUS.md before committing.
- STATUS.md is the snapshot a fresh context reads first. Keep it under 80 lines.
- On every commit, update STATUS.md and README.md to reflect the current state.
- Every public type, function, and module must have a `///` doc comment. `cargo doc` is the project documentation.
- Do NOT look at files outside this repo for reference.
- The plan is the spec. Work from the plan and AGENTS.md only.
