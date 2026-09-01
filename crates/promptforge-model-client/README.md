# promptforge-model-client

The PromptForge gateway's model client: an `OpenAI`-compatible chat
completions transport (`GatewayClient`), the wire types it exchanges, the
model catalog (`ModelCatalog`, `ModelDescriptor`, `ModelId`), and the
prompt-local binding vocabulary (`ModelBinding`, `ModelSet`, `ModelView`,
`ModelResolver`) the executor resolves `models.bind` declarations against.

The client holds only the gateway's URL and the shared key; the vendor
credential lives in the gateway, so a caller never sees it. Streaming is not
supported for completions. `subscribe_progress` consumes the gateway's
`GET /admin/progress` SSE stream as decoded `promptforge-progress` events.

Each `Completion` carries the call's metadata parsed from the response body:
the serving `model`, `usage` token accounting (with cached- and
reasoning-token details), llama.cpp `timings`, and vLLM `metrics`. The
metrics vocabulary (`Usage`, `LlamaTimings`, `VllmMetrics`, `ClientTiming`,
`CallMetrics`) is canonical in `promptforge-core-support` and re-exported at
this crate's root. A malformed metadata section degrades to `None` with a
`tracing` warning; it never fails the call.
