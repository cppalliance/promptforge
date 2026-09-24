# promptforge-model-client

The PromptForge model vocabulary: the chat-completions wire types a
`Chat` effect exchanges (`Message`, `ToolSchema`, `ToolCall`,
`Completion`, `CompletionResult`), the SSE reassembly that folds a
streamed body into one `Completion`, the model catalog (`ModelCatalog`,
`ModelDescriptor`, `ModelId`), and the prompt-local binding vocabulary
(`ModelBinding`, `ModelSet`, `ModelView`) the executor resolves model
declarations against. No transport: the HTTP client that sends a round to
the gateway is the harness's (`harness-models`).

A round is always streamed. The transport asks for
`stream_options.include_usage`, hands each SSE `data:` payload to the
`StreamAccumulator`, and invokes the caller's callback with each live
`StreamDelta` text or reasoning fragment; `finish` applies the one rule
set (a tool-call batch finished by `length` or `content_filter` fails
whole, so partial arguments never execute; an empty product is
`EmptyReply`) and produces the `Completion`.

Each `Completion` holds the call's metadata parsed from the stream:
the serving `model`, `usage` token accounting (with cached- and
reasoning-token details), llama.cpp `timings`, vLLM `metrics`, and the
`client_timing` (TTFT, mean inter-token latency, end-to-end) the
transport measured on its own clock. The metrics vocabulary (`Usage`,
`LlamaTimings`, `VllmMetrics`, `ClientTiming`, `CallMetrics`) is canonical
in `promptforge-api-types` and re-exported at this crate's root. A
malformed metadata section degrades to `None` with a `tracing` warning; it
never fails the call.
