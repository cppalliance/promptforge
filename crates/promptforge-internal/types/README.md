# promptforge-api-types

Small shared host-support primitives for the PromptForge runtime:
`untrusted` wraps untrusted external data in a nonce-guarded envelope,
`cancel` is the polled cancellation tree the engine observes, `event` is
the report-only `Event` vocabulary a run returns to its host, `emitter` is
the provenance-stamping `Emitter` every engine crate reports through, and
`metrics` is the model-call metrics vocabulary those events embed. The
crate declares no async runtime.
