# harness-models

This crate owns the harness's model transport: the HTTP client that performs the engine's `Chat` effects against the bound gateway, and the catalog fetch a host resolves model selections against. The dependency rules and the core invariants are in the `## Invariants` block of `src/lib.rs`; this file holds only what that block does not say.

- Other protocols use separate clients; this one speaks only the always-streaming `/chat/completions` SSE shape and `GET /v1/models`.
- The request body builder, the SSE reassembly, and the read loop (`read_body_capped`, `read_completion_stream` over a `ChunkSource`) are shared seams behind the `promptforge_api_runtime::model` public API; this crate never rebuilds the body shape, re-judges a turn, or grows its own copy of the byte cap, the `[DONE]` rule, or the timing arithmetic. It owns only what touches the wire: sending, the request timeout, the response as a chunk source, the clock it hands the read loop, and environment loading.
- Every `reqwest::Error` this crate erases into the internal error type (`Http`, `BackendBodyRead`) is boxed through `transport_source`, which applies the timeout marker, so `is_timeout` holds under every variant.
- The shared bearer key is wrapped in `SecretString` at the boundary; `Debug` redacts to a fixed marker so no presence or length signal leaks, and the key never appears in `Display` or error text.
- A keyless client is an explicit choice (`GatewayClient::keyless`, or `from_env` against a loopback URL); nothing here checks the endpoint's host on the caller's behalf.
- A backend error body is bounded and control-escaped before it is kept, and is exposed only through the opt-in `backend_body` accessor, never in `Display`. A success stream is refused once it exceeds the run's byte cap, before decoding.
