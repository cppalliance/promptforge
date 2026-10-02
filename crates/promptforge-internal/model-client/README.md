# promptforge-model-client

The PromptForge model vocabulary: the chat-completions wire types a
`Chat` effect exchanges (`Message`, `ToolSchema`, `ToolCall`,
`Completion`, `CompletionResult`), the SSE reassembly that folds a
streamed body into one `Completion`, the model catalog (`ModelCatalog`,
`ModelDescriptor`, `ModelId`), and the prompt-local binding vocabulary
(`ModelBinding`, `ModelSet`, `ModelView`) the executor resolves model
declarations against. The model catalog types are defined in
`promptforge-types` and re-exported by this crate's `model` module. No
transport: the HTTP client that sends a round to
the gateway is the Harness's (`harness-models`).

A round is always streamed. The transport asks for
`stream_options.include_usage`, hands each SSE `data:` payload to the
`StreamAccumulator`, and invokes the caller's callback with each live
`StreamDelta` text or reasoning fragment; `finish` applies the one rule
set (a tool-call batch finished by `length` or `content_filter` fails
whole, so partial arguments never execute; an empty product is
`EmptyReply`) and produces the `Completion`.

Each `Completion` holds the call's metadata parsed from the stream:
the serving `model`, and `metrics`, one `CallMetrics` built from `usage`
token accounting (with cached- and reasoning-token details), llama.cpp
`timings`, vLLM `metrics`, and the client timing (TTFT, mean inter-token
latency, end-to-end) the transport measured on its own clock. `metrics`
is absent when nothing was measured, and a stream with no usage chunk
has no `usage`. The read loop also attaches a `RawExchange`, the request
body the transport sent and the response body rebuilt from the chunks,
which the Engine copies into its debug capture events when a run turns
capture on. A completion built without a transport has neither. The
metrics vocabulary (`Usage`,
`LlamaTimings`, `VllmMetrics`, `ClientTiming`, `CallMetrics`) and
`StreamDelta` are canonical in `promptforge-types`; this crate uses them
from there and does not re-export them. A
malformed metadata section degrades to `None` with a diagnostic line that
the Engine reports as a `model_metadata_degraded` event; it never fails
the call.

A failed round is a `CompletionError`: a closed `CompletionErrorKind`
(`ContextOverflow`, `RateLimited`, `Timeout`, `Transport`, and the rest)
with retryability fixed per kind, a message that is the kind's fixed
phrase, and an opt-in `detail` for the provider's bounded, escaped text.
`classify_http_failure` reads a non-success status and body into one. A
poisoned model-set lock is not a model failure: `ModelView` returns the
one-purpose `ModelSetError` for it.
