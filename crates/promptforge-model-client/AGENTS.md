# promptforge-model-client

This crate owns the OpenAI-shaped Gateway model transport and model-binding vocabulary.

- This is a Gateway model client, not a universal transport. Other protocols use separate clients.
- The client does not depend on a parser, Lua runtime, store, observer, or executor. Executors adapt to it.
- Metrics vocabulary is canonical in `promptforge-core-support`. This crate parses responses into those types and never defines a parallel metrics model.
- Hidden cross-crate seams let executors reach non-host internals. They must not gain documented status without a design change.
