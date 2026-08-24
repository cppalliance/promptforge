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
- A25. Every model-facing section uses a prompt-declared model binding; hosts never choose a model implicitly.
- A26. Artifact credentials are read from process secrets at request time and are never persisted or logged.
- A30. Fan-out arms run with bounded concurrency, return in input order, and abort siblings on the first error.


- A42. External file I/O ends at the trusted host; prompts see only validated store paths.












- A63. The boot catalog defines available models; a required named profile selects the loaded subset while server identity stays fixed.
- A64. Endpoints bind by id to one shared dominion admission queue; local dominions also enforce complete VRAM co-residency budgets.

- A65. Clients and core speak one canonical protocol; per-model backend dialect translation exists only inside the gateway and grows by demonstrated need.
- A66. Streaming holds admission for its full lifetime, cancels upstream work on disconnect, and validates chunks without whole-body buffering.

- A67. Workbench logic lives in a local Rust server; the native executable is a thin webview shell so the server can embed elsewhere.
- A68. Workbench appends every chat request and response to ordered raw JSONL; the tape preserves history but promises no deterministic replay.

- A69. Missing Workbench configuration creates an editable user-directory TOML with defaults; environment interpolation is only an input within that file.
- A70. The gateway alone downloads, verifies, lists, and deletes model artifacts while streaming progress to clients.

## Principles

- Before adding configuration, public API, or resolution machinery, prefer sandboxed Lua, the run-scoped store, or the catalog when one already carries the work.
