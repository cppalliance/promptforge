# gateway-protocol

This crate owns the OpenAI wire protocol and the upstream abstraction: the wire types and their validation, the `Upstream` trait and `OpenAiUpstream`, the bounded HTTP client helpers, and the protocol-level error types.

- OpenAI wire protocol and upstream abstraction only: no local inference, no routing, no axum handlers.
- The crate never names gateway-local concepts (`LocalError`, profile switching, dominion queues); the `Upstream::shutdown` seam is typed on this crate's own `ShutdownError` so no edge points back into gateway code.
