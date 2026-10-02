# promptforge-model-client

The PromptForge model vocabulary: the chat-completions wire types a
`Chat` effect exchanges (`Message`, `ToolSchema`, `ToolCall`,
`Completion`, `CompletionResult`), the model catalog (`ModelCatalog`,
`ModelDescriptor`, `ModelId`), and the prompt-local binding vocabulary
(`ModelBinding`, `ModelSet`, `ModelView`) the executor resolves model
declarations against. The model catalog types are defined in
`promptforge-types` and re-exported by this crate's `model` module. No
transport and no wire parsing: the OpenAI wire code that builds a round's
request body and reads its streamed reply into a `Completion` lives in
`harness-gateway-client`.

A `Completion` is built with `Completion::from_result` and its `with_*`
builders, and each `ToolCall` with `ToolCall::from_parts`. These
validating constructors run the neutral reply checks (a nonblank call id
and name, object arguments, distinct ids in one turn, a non-empty batch),
so a completion a wire decoder built and one a Harness built by hand are
judged alike.

Each `Completion` holds the call's metadata a broker reported: the
serving `model`, and `metrics`, one `CallMetrics` built from `usage`
token accounting (with cached- and reasoning-token details), llama.cpp
`timings`, vLLM `metrics`, and the client timing (TTFT, mean inter-token
latency, end-to-end) the transport measured on its own clock. `metrics`
is absent when nothing was measured. A broker may also attach a
`RawExchange`, the request body it sent and the response body it read,
which the Engine copies into its debug capture events when a run turns
capture on. A completion built without a transport has neither. The
metrics vocabulary (`Usage`,
`LlamaTimings`, `VllmMetrics`, `ClientTiming`, `CallMetrics`) and
`StreamDelta` are canonical in `promptforge-types`; this crate uses them
from there and does not re-export them. A
malformed metadata section degrades to `None` with a diagnostic line that
the broker attaches and the Engine reports as a `model_metadata_degraded`
event; it never fails the call.

A failed round is a `CompletionError`: a closed `CompletionErrorKind`
(`ContextOverflow`, `RateLimited`, `Timeout`, `Transport`, and the rest)
with retryability fixed per kind, a message that is the kind's fixed
phrase, and an opt-in `detail` for the provider's bounded, escaped text.
`harness-gateway-client`'s `classify_http_failure` reads a non-success
status and body into one. A poisoned model-set lock is not a model
failure: `ModelView` returns the one-purpose `ModelSetError` for it.
