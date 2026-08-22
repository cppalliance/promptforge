# PromptForge architecture

## Identity

PromptForge is a Rust system for executing Markdown prompt pipelines and Lua agent programs. It ships a reusable executor, a command line interface, a separate inference gateway, and a desktop workshop for developers who author and run prompts against local or remote models.

## Components

- executor: parses and executes prompt pipelines and agent programs; depends on: gateway, store, Lua VM boundary, shared substrate
- gateway: lean server process that owns model routing, provider access, and local inference lifecycle; exposes protocol data rather than shared crate types; depends on: shared substrate
- CLI: thin shell adapter that supplies inputs and host resources to the executor; depends on: executor, gateway, store, shared substrate
- workshop UI: desktop authoring shell and server that host the executor in-process and attach to the gateway; depends on: executor, gateway, store, shared substrate
- store: run-scoped virtual filesystem contract with interchangeable memory and file backends; depends on: none
- Lua VM boundary: sandbox and coroutine bridge between prompt code and host capabilities; depends on: gateway, store, shared substrate
- shared substrate: cross-product progress, loopback discovery, protocol, and sidecar facilities; depends on: none

## Invariants

- A1. The gateway runs in a process separate from every caller.
- A2. The gateway is the sole holder of vendor and remote-service credentials.
- A3. The executor installs no process-global state.
- A5. The store confines every path to its configured backend root.
- A6. The Lua VM boundary exposes only host-installed capabilities.
- A10. Every model-chosen network destination is revalidated after DNS and on each redirect; private addresses are denied by default.
- A11. Every potentially unbounded model or tool loop has a finite explicit budget and fails visibly when exhausted.
- A12. Each section receives only capabilities it explicitly names; unknown names fail before the model turn.
- A15. Every section and fan-out arm gets a fresh model context; durable cross-run state crosses through the store or named payloads.
- A19. Runtime metadata is sealed: unknown reads and all author writes fail, and every field has an explicit refresh boundary.
- A25. Every model-facing section uses a prompt-declared model binding; hosts never choose a model implicitly.
- A26. Artifact credentials are read from process secrets at request time and are never persisted or logged.
- A27. Model tool-wire dialects are resolved from runtime evidence and applied at one normalization boundary; prompts remain dialect-agnostic.
- A30. Fan-out arms run with bounded concurrency, return in input order, and abort siblings on the first error.


- A42. External file I/O ends at the trusted host; prompts see only validated store paths.






- A52. Every untrusted string is guard-wrapped explicitly at the model-facing insertion boundary, independent of its origin.

- A55. Fan-out maps an explicit collection across isolated JSON values and returns each original member with its result.


- A58. Variable state rolls through one walk and its jumps; execute chains and fan-out arms receive isolated snapshots whose writes do not escape.
- A59. Distinct concurrent fan-out arms writing the same store path fail visibly; concurrent append remains legal.

- A60. Parse, model, and dialect failures are classified from explicit typed variants, never from user-controlled message text or wildcard fallbacks.

- A61. Immutable run state is shared across the execute subtree; each section entry owns a fresh frame with explicit construction and teardown.
- A62. Untrusted envelopes use one nonce per run: byte-stable within a run, unpredictable across runs, with escaping as the primary defense.

## Principles

- Before adding configuration, public API, or resolution machinery, prefer sandboxed Lua, the run-scoped store, or the catalog when one already carries the work.
