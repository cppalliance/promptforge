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
- build tooling: compile-time and packaging code linked into no runtime deliverable; depends on: none
- tool picker: deterministic catalog resolver that can shortlist or abstain but cannot execute effects; depends on: none

## Invariants

- A1. The gateway runs in a process separate from every caller.
- A2. The gateway is the sole holder of vendor and remote-service credentials.
- A3. The executor installs no process-global state.
- A4. The CLI and workshop UI delegate all prompt execution to the executor.
- A5. The store confines every path to its configured backend root.
- A6. The Lua VM boundary exposes only host-installed capabilities.
- A7. Runtime code never invokes build tooling.
- A8. Long-running operations report progress only through shared substrate.
- A9. Template expansion is single-pass lookup; computation stays in sandboxed Lua.
- A10. Every model-chosen network destination is revalidated after DNS and on each redirect; private addresses are denied by default.
- A11. Every repeated model or tool operation has a finite explicit budget and fails visibly when exhausted.
- A12. Each section receives only capabilities it explicitly names; unknown names fail before the model turn.
- A13. Untrusted tool output is marked automatically; capability and context isolation remain the security boundary.
- A14. Only prompts declaring a supported PromptForge major execute; missing or unsupported majors are refused.
- A15. Every section entry gets a fresh model context; only Lua chooses transitions, and state crosses through the store or explicit payloads.

## Principles

- Before adding configuration, public API, or resolution machinery, prefer sandboxed Lua, the run-scoped store, or the catalog when one already carries the work.
- Keep model-facing prose in Markdown and programmable logic in Lua.
- Treat each binary as a product boundary and reserve build features for real toolchain or native-build constraints.

## Thresholds

- god_atfd: 5
- god_wmc: 47
- cluster_params: 3
- cluster_sites: 2
- large_diff_files: 6
- large_diff_lines: 400

## Decided against

- 2026-09-04 Hosting the workshop UI inside the gateway was proposed and declined; the gateway remains separate.
