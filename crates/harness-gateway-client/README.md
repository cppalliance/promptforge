# harness-gateway-client

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](../../LICENSE)

The standard way a Host talks to the PromptForge Gateway. It holds the OpenAI chat-completions wire code: it turns a `Chat` effect into the one request body every round sends, reads the streamed reply into the `Completion` that answers the effect, and turns a failed response into the `CompletionError` the round carries. It sits beside `harness` at the `crates/` root, and like every Harness crate it depends only on `promptforge`. Nothing here opens a connection or reads a clock: the caller sends the body, supplies the response as a `ChunkSource`, and hands the read loop its clock.

## Public surface

- `build_request_body` - the chat-completions JSON body for a `Chat` effect's messages, tools, and options. Every body streams and asks for the final usage chunk, wraps each tool in the OpenAI function shape, and adds `temperature`, `max_tokens`, and `chat_template_kwargs.enable_thinking` only when set.
- `ChunkSource` - a response body read one chunk at a time; the only I/O the read helpers touch. A read failure is the caller's own `Timeout` or `Transport` error, returned unchanged.
- `read_completion_stream` - reads the SSE reply to its `[DONE]` sentinel under a byte cap, forwards each text or reasoning delta live, and folds the stream into a `Completion` with the round's `CallMetrics` (the backend's `usage`, llama.cpp `timings`, vLLM `metrics`, and the client timing measured on the caller's clock) and the `RawExchange` of the request sent and the response rebuilt.
- `read_body_capped` - reads a body the caller decodes whole, such as a non-success status's error body or a model list, refusing it past a byte cap.
- `escape_controls` - bounds a backend error body to a character count and escapes its control characters, so it cannot forge log lines.
- `classify_http_failure` and `classify_stream_error` - turn a non-success status and its escaped body, or an in-stream error envelope, into a `CompletionError` of the right kind, keeping the body as the opt-in `detail` and never in the message.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
