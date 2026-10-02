# harness-gateway-client

This crate owns the OpenAI chat-completions wire code a Host uses to talk to the PromptForge Gateway. The dependency rules are in the `## Invariants` block of `src/lib.rs`; this file holds the wire invariants that block does not say.

- One request-body builder. `build_request_body` is the only place the chat-completions body is shaped, so every transport sends the same JSON for one `Chat` effect. Never build or patch a body elsewhere.
- One read loop. `read_completion_stream` and `read_body_capped` over a caller's `ChunkSource` hold the byte cap, the `[DONE]` rule, and the timing arithmetic once. The SSE scanner, the accumulator, and the body walk in `wire/parse.rs` stay crate-private, reached only through the read loop, and no transport grows its own copy.
- The `[DONE]` rule. A stream that ends without the `data: [DONE]` sentinel was cut off and fails as `MalformedResponse`; it never passes for a complete turn. A tool-call batch finished by `length` or `content_filter` fails whole, so partial arguments never execute.
- The byte cap. A success stream is refused once its raw bytes, framing included, pass the cap, before decoding. A body read whole is refused on an advertised length over the cap, and its running total stops at the cap either way.
- Bounded, control-escaped backend bodies. A backend body passes through `escape_controls` before it reaches a `CompletionError`, and it is kept only as the opt-in `detail`, never in the message or `Display`. Messages are the kind's fixed phrase, extended only with specifics this crate wrote.
- Public constructors only. Every `Completion` and `ToolCall` is built through `Completion::from_result` and its `with_*` builders and `ToolCall::from_parts`, so the Engine's neutral reply checks run on every decoded turn. Error kinds, error text, and delta order are pinned by the tests; change them only on purpose.
- No I/O and no clock. The crate opens no connection, names no HTTP client, and reads no clock; the caller supplies the chunks and the `Instant`s.
