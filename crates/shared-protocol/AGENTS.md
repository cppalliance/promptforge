# shared-protocol

This crate owns the OpenAI wire protocol, bounded client behavior, and the upstream abstraction.

- Local inference, routing, and HTTP handlers stay in their owning crates.
- Shared protocol errors do not name Gateway-local concepts. Upstream shutdown uses this crate's error vocabulary so no dependency points back into Gateway code.
