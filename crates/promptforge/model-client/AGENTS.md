# promptforge-model-client

This crate owns the model vocabulary: the chat-completions wire types, the SSE reassembly, and the model-binding vocabulary. It owns no transport.

- No HTTP. The crate never opens a connection, names an HTTP client, or reads the environment. `reqwest` and `url` are not dependencies; a change that needs them belongs in `harness-models`, which reaches this vocabulary through the `promptforge-api-runtime` public API.
- The wire types (`Message`, `ToolSchema`, `ToolCall`, `Completion`, `CompletionResult`) are what a `Chat` effect carries and what its answer carries back. They are `#[non_exhaustive]`; the constructors in `client/wire-canned.rs` are the one way to build them from outside.
- The request body builder, the SSE reassembly (`SseScanner`, `StreamAccumulator`, `finish`), and the read loop over a transport's `ChunkSource` (`read_body_capped`, `read_completion_stream`) are `#[doc(hidden)]` seams shared by every transport, so one request shape leaves, one byte cap and sentinel rule bound every body, and one rule set judges every turn, streamed or buffered. The strict turn rules stay in `normalize`; the accumulator only reassembles. A transport contributes only its chunks and its clock: this crate reads no clock, and the read loop measures `ClientTiming` against the `Instant`s the transport hands it.
- `CompletionError` is the failure a round reports. Its `#[doc(hidden)]` substrate (`Error`, `Timeout`) is public only so the runtime maps it verbatim and a transport can construct it; it is not host API.
- Metrics vocabulary is canonical in `promptforge-api-types`. This crate parses responses into those types and never defines a parallel metrics model.
- The crate does not depend on a parser, Lua runtime, store, observer, or executor. Executors adapt to it.
- Hidden cross-crate seams let executors and transports reach non-host internals. They must not gain documented status without a design change.
