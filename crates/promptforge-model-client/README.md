# promptforge-model-client

The PromptForge gateway's model client: an `OpenAI`-compatible chat
completions transport (`GatewayClient`), the wire types it exchanges, the
model catalog (`ModelCatalog`, `ModelDescriptor`, `ModelId`), and the
prompt-local binding vocabulary (`ModelBinding`, `ModelSet`, `ModelView`,
`ModelResolver`) the executor resolves `models.bind` declarations against.

The client holds only the gateway's URL and the shared key; the vendor
credential lives in the gateway, so a caller never sees it. `complete` is
the one completion method and always streams SSE internally: it requests
`stream_options.include_usage`, accumulates the deltas into one
`Completion`, and invokes the caller's callback with each live
`StreamDelta` text or reasoning fragment (a caller with no use for deltas
passes a no-op closure). A tool-call batch finished by `length` or
`content_filter` fails whole, so partial arguments never execute.
`subscribe_progress` consumes the gateway's `GET /admin/progress` SSE
stream as decoded `shared-progress` events.

Each `Completion` carries the call's metadata parsed from the stream:
the serving `model`, `usage` token accounting (with cached- and
reasoning-token details), llama.cpp `timings`, vLLM `metrics`, and a
`client_timing` (TTFT, mean inter-token latency, end-to-end) measured on
the client's own clock. The metrics vocabulary (`Usage`, `LlamaTimings`,
`VllmMetrics`, `ClientTiming`, `CallMetrics`) is canonical in
`promptforge-core-support` and re-exported at this crate's root. A
malformed metadata section degrades to `None` with a `tracing` warning; it
never fails the call.
