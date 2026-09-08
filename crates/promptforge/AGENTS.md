# promptforge

This crate is the PromptForge library product's integrator-facing facade.

- Keep it facade-only. Do not add logic, new types, wrappers, or substrate dependencies here.
- The `pipeline` and `agent` modules define its public vocabulary. Do not grow a parallel API around the underlying executors.
