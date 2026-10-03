# PromptForge architecture

## Identity

PromptForge is a Rust system for executing Markdown prompt programs. It ships a reusable sans-I/O executor (the Engine), the Harness that runs it, a command line interface, an inference gateway, and a desktop workshop for developers who author and run prompts against local or remote models.

## Components

- Engine (executor): a deterministic state machine that parses and executes Markdown prompt programs; given the same context and the same sequence of answers it produces the same effects, events, and ids; it exchanges every effect and event with the Harness as a serializable value through its stepping API, `Run::new`, `step`, `resume`, and `cancel`, so every file, model reply, and clock reading reaches it as the Harness's answer; depends on: Lua VM boundary, shared substrate
- Harness: steps the Engine and performs its effects, with a performer for each tool-call and timer effect and each Vfs effect answered inline in its effect loop; serves every model round and resolves each run's model through the `InferenceBroker` the Host passes to `Harness::new`, and holds no model HTTP client and no gateway binding; owns the tokio runtime, agent discovery and sessions with their input waits, supervisor, and in-memory transcripts; activates each run's capabilities from the registry and services the Host supplies at construction, because the Host registers every capability, first-party ones included, and supplies every service, and the Harness builds none; writes every effect, answer, and event of a run, in order, to the recorder the Host supplies; its public surface is `harness`, `harness-gateway-client`, and `harness-web`; depends on: executor (only through `promptforge`; no gateway or shared crate)
- Host: the application that builds the Harness and launches runs, such as Workshop, Papergate, a CLI, or a batch job; it makes every policy decision, such as the selected model, whether someone answers `input.ask()`, and when to cancel; depends on: Harness
- gateway: independent server process that owns model routing, provider access, and local inference lifecycle; exposes protocol data and discovery; depends on: shared substrate
- CLI: a Host; a thin shell adapter that supplies a run's inputs and real files to the executor; depends on: executor, gateway, shared substrate
- workshop UI: desktop authoring shell and in-process server that drive agent sessions through `harness` and attach over the gateway protocol; persists user-scoped UI state through `workshop-user-state` (one JSON file in the state directory) and workspace-scoped UI state through the `.pfwork` workspace file, and keeps the runs the Harness records in a Turso run log through `workshop-run-log`; depends on: Harness, gateway, shared substrate
- VFS layer: canonical paths, claims, routing, memory and real-filesystem backends, the policy gate, the declared store root and the store view, all in `promptforge-vfs`; depends on: none
- Lua VM boundary: sandbox and coroutine bridge between prompt code and the Engine globals; every suspending author function is a Lua shim that yields a request value the executor answers, so the boundary itself performs no I/O and names no transport; depends on: shared substrate (model wire vocabulary only, no gateway crate)
- shared substrate: cross-product progress, loopback discovery, protocol, and gateway discovery facilities, plus the error-source wrappers (`shared-error-source`) that the workshop and gateway families wrap their third-party causes through, one per wrapped error behind its own feature (Harness crates may not depend on it); depends on: none

## Invariants

- A1. The Gateway binds its HTTP listener and reports readiness before it starts model downloads or model processes; slow provisioning runs afterward through the Gateway command queue.
- A2. Vendor credentials remain inside the Gateway process; Workshop and CLI reach credentialed model providers only through server-side Gateway relays that never expose vendor bearer keys to browser or Lua code.
- A3. The fetch tool in `harness-web` revalidates every model- or tool-selected URL and resolved address on each redirect, and denies non-global addresses unless fetch configuration grants an exact host-and-address exception.
- A4. The Workshop server rejects cross-site requests, non-loopback Host values, and WebSocket origins outside its allowed loopback origins; the Workshop webview accepts in-view navigation only to its exact boot origin.
- A5. The Gateway's local model set is fixed for the process lifetime; profile and local-model changes persist and report `restart_required`, and remote routing changes replace the routing table atomically without draining.
- A6. The executor neutralizes chat-template control delimiters in untrusted tool and Lua text, but never rewrites assistant replay or tool-call wire payloads.
- A7. The Workshop shell grants each Tauri capability to one named window and the in-process server's exact bound origin, never a wildcard port.
- A8. The Lua VM boundary accepts scheduler state changes only from typed `Request` variants yielded by the installed shim; direct or malformed yields fail without changing scheduler state.
- A9. The Lua VM boundary exposes the Engine globals as namespace functions over plain values (`models.*`, `tools.*`, `store.*`); handles are frozen, inspectable, and methodless, with an optional leading handle argument selecting an explicit binding. The chainable `messages.new()` builders are the sole deliberate exception.
