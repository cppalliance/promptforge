# PromptForge architecture

## Identity

PromptForge is a Rust system for executing Markdown prompt pipelines and Lua agent programs. It ships a reusable executor, a command line interface, an inference gateway, and a desktop workshop for developers who author and run prompts against local or remote models.

## Components

- executor: parses and executes prompt pipelines and agent programs; depends on: gateway, store, Lua VM boundary, shared substrate
- gateway: server that owns model routing, provider access, and local inference lifecycle; it may cohost the Workshop listener but exposes protocol data rather than shared crate types; depends on: shared substrate
- CLI: thin shell adapter that supplies inputs and host resources to the executor; depends on: executor, gateway, store, shared substrate
- workshop UI: desktop authoring shell and server that host the executor in-process and speak the gateway protocol even when cohosted; depends on: executor, gateway, store, shared substrate
- store: run-scoped virtual filesystem contract with interchangeable memory and file backends; depends on: none
- Lua VM boundary: sandbox and coroutine bridge between prompt code and host capabilities; depends on: gateway, store, shared substrate
- shared substrate: cross-product progress, loopback discovery, protocol, and sidecar facilities; depends on: none

## Invariants

- A2. The gateway is the sole holder of vendor and remote-service credentials.
- A3. The executor installs no process-global state.
- A5. The store confines every path to its configured backend root.
- A6. The Lua VM boundary exposes only host-installed capabilities.
- A10. Every model-chosen network destination is revalidated after DNS and on each redirect; private addresses are denied by default.
- A11. Every potentially unbounded model or tool loop has a finite explicit budget and fails visibly when exhausted.
- A12. Each section receives only capabilities it explicitly names; unknown names fail before the model turn.
- A15. Every section and fan-out arm gets a fresh model context; durable cross-run state crosses through the store or named payloads.

- A64. Endpoints bind by id to one shared dominion admission queue; local dominions also enforce complete VRAM co-residency budgets.

- A65. Clients and core speak one canonical protocol; per-model backend dialect translation exists only inside the gateway and grows by demonstrated need.

- A75. Gateway clients bound connection and buffered-request waits; streaming uses lifecycle cancellation instead of a whole-request deadline.

- A77. Workshop file APIs canonicalize every path and confine access to roots explicitly granted for the current session.

- A78. Each Agent panel owns an isolated chat and plugin lifecycle; only transport and selected-model services are shared.

- A79. Model stream channels retain their semantics end to end; reasoning is neither dropped nor folded into answer content.

- A80. Workshop UI dependencies flow from ui to services to base; main.ts alone composes layers and builds reject reverse edges.

- A82. Chat replies are ordered durable per-request traffic; status, catalog, and interim transcription are complete ephemeral snapshots.

- A83. The server owns complete Workshop state snapshots; clients send commands, render snapshots, and do not derive readiness.

- A84. Gateway and Workshop may share a process but retain their loopback protocol boundary; headless builds omit Workshop.

- A85. Workshop APIs reject cross-site traffic and unapproved WebSocket origins; writes to user files are atomic.

- A86. A merged process completes Workshop shutdown before signaling gateway shutdown.

- A87. Lua suspension crosses a closed yield/resume protocol; only structural requests alter scheduler state.

## Principles

- Before adding configuration, public API, or resolution machinery, prefer sandboxed Lua, the run-scoped store, or the catalog when one already carries the work.
