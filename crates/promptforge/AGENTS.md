# promptforge

This crate is the promptforge library product's integrator-facing facade: it re-exports `pipeline::run` and `agent::run` with docs and never grows logic or types of its own.

- Facade only. Dependencies are `promptforge-core` and `promptforge-agent` only; never add logic, new types, wrappers, or substrate dependencies here - integrators who need substrate types depend on those crates directly.
- Re-exports carry `///` docs; the facade vocabulary (`pipeline`, `agent`) is the public surface - do not rename modules or grow a parallel API around the underlying executors.
