# PromptForge architecture

## Identity

PromptForge is a Rust system for executing Markdown prompt programs. It ships a reusable sans-I/O executor, a harness that hosts it, a command line interface, an inference gateway, and a desktop workshop for developers who author and run prompts against local or remote models.

## Components

- executor: a deterministic state machine that parses and executes Markdown prompt programs; given the same context and the same sequence of answers it produces the same effects, events, and ids; performs no I/O, reads no clock, and holds no host trait objects; its host interface is `Run::new`, `step`, `resume`, and `cancel`, exchanging effects and events as serializable values; depends on: store, Lua VM boundary, shared substrate
- harness: the executor's only production host; owns the tokio runtime, one performer per effect kind, the model HTTP client, the capability registry and first-party capabilities, agent discovery and sessions with their input waits and supervisor, and the append-only Turso run log of every effect, answer, and event; its public surface is `harness-api`, and it receives the gateway binding as data pushed across that door; depends on: executor, gateway (public protocol and discovery crates only), store, shared substrate
- gateway: independent server process that owns model routing, provider access, and local inference lifecycle; exposes protocol data and discovery; depends on: shared substrate
- CLI: thin shell adapter that supplies inputs and host resources to the executor; depends on: executor, gateway, store, shared substrate
- workshop UI: desktop authoring shell and in-process server that drive agent sessions through `harness-api` and attach over the gateway protocol; persists user-scoped UI state through `workshop-user-state` (one JSON file in the state directory) and workspace-scoped UI state through the `.pfwork` workspace file; depends on: harness, gateway, store, shared substrate
- store: run-scoped Store facade over the VFS layer, built with `Store::new(&access)`; depends on: VFS layer
- VFS layer: canonical paths, claims, routing, memory and host backends, and the policy gate, all in `promptforge-vfs`; depends on: none
- Lua VM boundary: sandbox and coroutine bridge between prompt code and host capabilities; every suspending author function is a Lua shim that yields a request value the executor answers, so the boundary itself performs no I/O and names no transport; depends on: store, shared substrate (model wire vocabulary only, no gateway crate)
- shared substrate: cross-product progress, loopback discovery, protocol, and gateway discovery facilities, plus the error-source wrappers (`shared-error-source`) that every family wraps its third-party causes through, one per wrapped error behind its own feature; depends on: none

## Invariants

- A1. The Gateway binds its HTTP listener and reports readiness before it starts model downloads or model processes; slow provisioning runs afterward through the Gateway command queue.
- A2. Vendor credentials remain inside the Gateway process; Workshop and CLI reach credentialed model providers only through server-side Gateway relays that never expose vendor bearer keys to browser or Lua code.
- A3. `harness-webfetch` revalidates every model- or tool-selected URL and resolved address on each redirect, and denies non-global addresses unless fetch configuration grants an exact host-and-address exception.
- A4. The Workshop server rejects cross-site requests, non-loopback Host values, and WebSocket origins outside its allowed loopback origins; the Workshop webview accepts in-view navigation only to its exact boot origin.
- A5. The Gateway's local model set is fixed for the process lifetime; profile and local-model changes persist and report `restart_required`, and remote routing changes replace the routing table atomically without draining.
- A6. The executor neutralizes chat-template control delimiters in untrusted tool and Lua text, but never rewrites assistant replay or tool-call wire payloads.
- A7. The Workshop shell grants each Tauri capability to one named window and the in-process server's exact bound origin, never a wildcard port.
- A8. The Lua VM boundary accepts scheduler state changes only from typed `Request` variants yielded by the installed shim; direct or malformed yields fail without changing scheduler state.
- A9. The Lua VM boundary exposes host capabilities as namespace functions over plain values (`models.*`, `tools.*`, `store.*`); handles are frozen, inspectable, and methodless, with an optional leading handle argument selecting an explicit binding. The chainable `messages.new()` builders are the sole deliberate exception.
