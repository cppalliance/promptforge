# promptforge-core-support

Small shared host-support primitives for the PromptForge runtime:
`untrusted` wraps untrusted external data in a nonce-guarded envelope,
`cancel` is the cooperative cancellation handle and task-local scope a run
observes, and `observe` is the report-only `Observer`/`Observation`
vocabulary a run reports its progress through.
