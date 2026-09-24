# promptforge-store

This crate owns the run-scoped virtual filesystem boundary.

- The virtual filesystem does not depend on an executor, Lua runtime, or tool provider. Consumers adapt to this crate. The one exception is the doctest-only `promptforge` dev-dependency the root `AGENTS.md` excepts: its doc examples alone compile against the facade, and no unit test, integration test, or bench imports it.
- Hidden fanout and test seams are not host API and must not gain documented status without a design change.
