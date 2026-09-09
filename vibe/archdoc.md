# PromptForge architecture

## Identity

PromptForge is a Rust system for executing Markdown prompt pipelines and Lua agent programs. It ships a reusable executor, a command line interface, an inference gateway, and a desktop workshop for developers who author and run prompts against local or remote models.

## Components

- executor: parses and executes prompt pipelines and agent programs; depends on: gateway, store, Lua VM boundary, shared substrate
- gateway: independent server process that owns model routing, provider access, and local inference lifecycle; exposes protocol data and discovery; depends on: shared substrate
- CLI: thin shell adapter that supplies inputs and host resources to the executor; depends on: executor, gateway, store, shared substrate
- workshop UI: desktop authoring shell and in-process server that host the executor and attach over the gateway protocol; depends on: executor, gateway, store, shared substrate
- store: run-scoped virtual filesystem contract with interchangeable memory and file backends; depends on: none
- Lua VM boundary: sandbox and coroutine bridge between prompt code and host capabilities; depends on: gateway, store, shared substrate
- shared substrate: cross-product progress, loopback discovery, protocol, and sidecar facilities; depends on: none

## Invariants

- A1. Reuse existing Lua, store, catalog, configuration, and protocol mechanisms before adding new machinery.
- A2. Give each credential, connection record, lifecycle, and persisted state exactly one owning subsystem.
- A3. Keep dependency direction explicit: higher layers depend on lower abstractions, never the reverse.
- A4. Inject only explicitly named capabilities and reject unknown capabilities before execution.
- A5. Bound every loop, queue, wait, stream, retry, and tool invocation.
- A6. Validate capabilities and semantics before queue admission or side effects.
- A7. Publish persisted configuration and live state atomically.
- A8. On failure or cancellation, preserve the last valid state and expose pending work explicitly.
- A9. Transfer durable state only through typed payloads or the run-scoped store.
- A10. Give each section, task, and fan-out arm fresh execution context.
- A11. Propagate cancellation to descendants only, never ancestors or siblings.
- A12. Keep control-plane readiness independent from slow model provisioning.
- A13. Preserve usable routing during preparation and exclude inference only during bounded cutover.
- A14. Fail visibly when budgets, capabilities, dialects, or provisioning requirements are unsatisfied.
- A15. Keep event history lossless and project it into model context deliberately.
- A16. Revalidate model-selected network destinations after DNS resolution and every redirect.
- A17. Deny private network destinations by default.
- A18. Canonicalize file paths and confine them to explicitly granted roots.
- A19. Keep vendor and remote-service credentials inside the gateway.
- A20. Let only the owning service create or mutate its connection record.
- A21. Require clients to validate process identity, health, and authority before attachment.
- A22. Reject cross-site requests and unapproved WebSocket origins.
- A23. Grant desktop capabilities per window and exact origin using least privilege.
- A24. Sanitize model-authored markup at the final DOM insertion boundary.
- A25. Neutralize control delimiters in untrusted text without rewriting model-generated wire payloads.
- A26. Route bounded third-party model content through the gateway rather than credentialed browser sources.
- A27. Hold stream permits until body termination and expire stalled reads.
- A28. Keep suspension protocols closed and typed so only structural requests alter scheduler state.
- A29. Restrict embedded navigation to the exact boot origin.
- A30. Write user-controlled files atomically.
