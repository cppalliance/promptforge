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
- A11. Every repeated model or tool operation has a finite explicit budget and fails visibly when exhausted.
- A12. Each section receives only capabilities it explicitly names; unknown names fail before the model turn.
- A15. Every section and fan-out arm gets a fresh model context; only explicit Lua chooses transitions, and state crosses through the store or named payloads.
- A19. Runtime metadata is sealed: unknown reads and all author writes fail, and every field has an explicit refresh boundary.
- A25. Every model-facing section uses a prompt-declared model binding; hosts never choose a model implicitly.
- A26. Artifact credentials are read from process secrets at request time and are never persisted or logged.
- A27. Model tool-wire dialects are resolved from runtime evidence and applied at one normalization boundary; prompts remain dialect-agnostic.
- A28. Tool-call accounting is scoped to one VM and prompt alias; unscoped tool names fail instead of dispatching.
- A29. Local lane concurrency is the single authority for gateway admission and backend parallel slots.
- A30. Fan-out arms run concurrently, return in input order, and abort siblings on the first error.
- A32. Search yields sanitized, source-diverse leads; fetch remains the boundary for page content.
- A33. Alternating blocks share one section conversation; non-final prose runs one round and final prose owns the full tool loop.
- A34. A subroutine call starts a fresh VM and returns; a transfer clears context, stops the caller, and does not return.
- A35. The H1 program executes exactly once with live host resolution; no separate declaration or bind replay phase exists.

- A37. A clean workspace build requires no secret environment variable or manually provisioned artifact.




- A41. Store persistence and root selection are caller policy; the executor is backend-agnostic.

- A42. External file I/O ends at the trusted host; prompts see only validated store paths.

## Principles

- Before adding configuration, public API, or resolution machinery, prefer sandboxed Lua, the run-scoped store, or the catalog when one already carries the work.
