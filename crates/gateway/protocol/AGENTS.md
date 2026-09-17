# gateway-protocol

This gateway crate owns the OpenAI wire protocol, bounded client behavior, and the upstream abstraction.

- Local inference, routing, and HTTP handlers stay in their owning crates.
- Protocol errors do not name local-inference concepts. Upstream shutdown uses this crate's error vocabulary so every gateway consumer shares one error surface.
