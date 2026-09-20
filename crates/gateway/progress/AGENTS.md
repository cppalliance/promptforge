# gateway-progress

This crate owns the gateway's live-activity hub: a busy flag and one line of producer-owned text, published as `gateway_api_types::Progress` over a `watch` channel.

- A private gateway family crate under `crates/gateway/`; only gateway crates depend on it. The Workshop and the harness consume the `Progress` wire type from `gateway-api-types` and never this machinery.
- Depends on `gateway-api-types` (the wire type), `tokio` `sync`, and nothing else in the workspace. Hosts own forwarding tasks; this crate does not spawn work, block a runtime, or log.
- Producers call `ProgressHub::begin(text)` and hold the returned `Activity` for the work's lifetime, updating it with `set_text` and dropping it on every exit path. Failure is not a progress state: the producer logs it and returns the error.
- The snapshot is `busy = any activity live`, `text = the newest live activity's text`. The `watch` channel keeps only the latest snapshot; there is no replay, no event history, and no fractions, weights, or hierarchy. A producer that wants a percentage formats it into the text.
- Activity text is user-visible in every status consumer; a producer never places a credential in it.
