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




- A77. Workshop file APIs canonicalize every path and confine access to roots explicitly granted for the current session.







- A85. Workshop APIs reject cross-site traffic and unapproved WebSocket origins; writes to user files are atomic.


- A87. Lua suspension crosses a closed yield/resume protocol; only structural requests alter scheduler state.

- A88. Cached artifacts skip hashing only when digest, size, and mtime match in an operator-trusted cache.

- A89. Native code compiles only during build or packaging; runtime only verifies, stages, and launches its artifacts.

- A90. Runtime-agnostic contract crates own shared vocabulary; providers and lifecycle crates depend inward.

- A91. Human boot config, machine state, view layout, and append-only tape remain separate stores.

- A92. Workshop sessions own transport and multiplexing; chat execution stays behind a replaceable adapter.

- A93. Operation-scoped progress trees attach to a process hub; owners schedule, producers report, renderers format.

- A94. Config writes enter validated persistent shadows; only Apply promotes them and Revert removes them.

- A95. Untrusted tool and Lua text neutralizes known control delimiters; model-generated wire payloads stay unchanged.

- A96. Browser UIs obtain bounded third-party model content through the gateway, never credentialed sources directly.

- A97. Per-crate guides are source truth; mdBook and flat guides assemble them without duplicate narratives.

- A98. Publishable crates have registry-resolvable dependency closure and release in dependency order.

## Principles

- Before adding configuration, public API, or resolution machinery, prefer sandboxed Lua, the run-scoped store, or the catalog when one already carries the work.
