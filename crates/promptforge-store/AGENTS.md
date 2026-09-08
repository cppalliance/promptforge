# promptforge-store

This crate owns the run-scoped virtual filesystem boundary.

- The virtual filesystem does not depend on an executor, Lua runtime, or tool provider. Consumers adapt to this crate.
- Hidden fanout and test seams are not host API and must not gain documented status without a design change.
