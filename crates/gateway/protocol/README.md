# Gateway wire protocol

This crate defines the OpenAI-shaped wire types the gateway exchanges with clients and backends, and it validates them at the trust boundary. It also provides the upstream abstraction and the shared HTTP client policy that every backend call goes through. It holds no routing, no local inference, and no server handlers.
