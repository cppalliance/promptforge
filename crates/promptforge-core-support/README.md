# promptforge-core-support

Small shared host-support primitives for the PromptForge runtime:
`untrusted` wraps untrusted external data in a nonce-guarded envelope,
`cancel` is the cooperative cancellation handle and task-local scope a run
observes, `observe` is the report-only `Observer`/`Observation` vocabulary a
run reports its progress through, and `events` is the canonical metrics and
runtime-event vocabulary with the read-side `EventLog` a host may supply as
a run input.
