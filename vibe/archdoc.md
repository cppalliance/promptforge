# PromptForge architecture

## Identity

PromptForge is a Rust system for executing Markdown prompt pipelines and Lua agent programs. It ships a reusable executor, a command line interface, an inference gateway, and a desktop workshop for developers who author and run prompts against local or remote models.

## Components

- executor: parses and executes prompt pipelines and agent programs; depends on: gateway, store, Lua VM boundary, shared substrate
- gateway: independent server process that owns model routing, provider access, and local inference lifecycle; exposes protocol data and discovery; depends on: shared substrate
- CLI: thin shell adapter that supplies inputs and host resources to the executor; depends on: executor, gateway, store, shared substrate
- workshop UI: desktop authoring shell and in-process server that host the executor and attach over the gateway protocol; depends on: executor, gateway, store, shared substrate
- store: run-scoped Store facade over the VFS layer, exposed as `vfs.store(&access)`; depends on: VFS layer
- VFS layer: canonical paths, claims, routing, and memory and host backends (`shared-vfs`), plus the policy gate (`promptforge-vfs`); depends on: none
- Lua VM boundary: sandbox and coroutine bridge between prompt code and host capabilities; depends on: gateway, store, shared substrate
- shared substrate: cross-product progress, loopback discovery, protocol, and sidecar facilities; depends on: none

## Invariants

- A1. The Gateway binds its HTTP listener and reports readiness before it starts model downloads or model processes; slow provisioning runs afterward through the Gateway command queue.
- A2. Vendor credentials remain inside the Gateway process; Workshop and CLI reach credentialed model providers only through server-side Gateway relays that never expose vendor bearer keys to browser or Lua code.
- A3. `promptforge-webfetch` revalidates every model- or tool-selected URL and resolved address on each redirect, and denies non-global addresses unless fetch configuration grants an exact host-and-address exception.
- A4. The Workshop server rejects cross-site requests, non-loopback Host values, and WebSocket origins outside its allowed loopback origins; the Workshop webview accepts in-view navigation only to its exact boot origin.
- A5. During a Gateway profile switch, the previous routing table serves until a bounded in-flight drain completes; after cutover, a selected model that is not ready returns an explicit loading error.
- A6. The executor neutralizes chat-template control delimiters in untrusted tool and Lua text, but never rewrites assistant replay or tool-call wire payloads.
- A7. The Workshop shell grants each Tauri capability to one named window and the in-process server's exact bound origin, never a wildcard port.
- A8. The Lua VM boundary accepts scheduler state changes only from typed `Request` variants yielded by the installed shim; direct or malformed yields fail without changing scheduler state.
- A9. The Lua VM boundary exposes host capabilities as namespace functions over plain values (`models.*`, `tools.*`, `store.*`); handles are frozen, inspectable, and methodless, with an optional leading handle argument selecting an explicit binding. The chainable `messages.new()` builders are the sole deliberate exception.
