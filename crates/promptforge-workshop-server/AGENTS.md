# Workshop Server Rules

These rules bind `crates/promptforge-workshop-server`. The repo-root AGENTS.md applies on top; the embedded UI has its own AGENTS.md under `ui/`. Rules here are target-state: refactor-era code lands in this shape.

## Two-zone error policy

The process has two zones with opposite failure postures. The boundary: config load plus server construction is zone one; request and session handling is zone two.

- Zone one (construction/startup): return rich errors to the host. This crate is embeddable, so it never panics for configuration, binding, asset, or initialization failures; a misconfigured process must not limp into serving, but the host decides how the failure surfaces. Binary entry points may convert a returned error to a failing exit status.
- Zone two (steady state): never panic, and never `unwrap` anything a client sent. Errors are values: error frames, 4xx/5xx responses, status-bus reports, or logged degradation. A lock poisoned by a panicking peer recovers the value rather than wedging the process.

Degrade-not-crash features (voice provisioning, gateway outages) are zone two by definition: absence of a capability is a state, not an error.

## WebSocket session model

One task owns each socket: a single `select!` loop reads and writes the same socket handle. No outbox channel, no writer task, no session registry. Durable messages deliver via `Notify` plus a per-client cursor and coalesce; ephemeral messages go through a bounded broadcast and drop on lag. Malformed inbound frames are logged and skipped, or close the connection with a policy code - never a panic. Each endpoint owns its socket, task, channels, protocol policy, and cleanup. Protocol-neutral helpers may be extracted inside an endpoint when they reduce current code; promote one across endpoints only after a second production consumer exists - never share hypothetical reuse. The session owns transport and multiplexing, not chat execution: direct gateway execution is the current adapter, not the session architecture.

Carve-out: agent sessions (`session_agents`) keep a session registry, because agent sessions survive socket disconnect by design - sockets attach and detach, reconnect replays the persisted event log and re-announces unresolved waits. The no-session-registry rule governed per-request relay work, where every held resource belonged to one socket; it stands for every other endpoint.

## Delivery contract

Every pushed message type is classified in the protocol module as durable or ephemeral; no message type ships unclassified. Durable state is recoverable from retained state or a cursor, and consumers tolerate duplicate delivery. Ephemeral snapshots may coalesce or drop under lag; the latest complete snapshot is resent on reconnect.

## Drop-guard cancellation

Work held on behalf of a client - a gateway completion, a whisper job, a tape span - is wrapped in a guard that cancels on disconnect. A resource that still needs a manual cleanup call is a wrong factoring.

## Embedding and process hygiene

This crate runs inside other binaries. Never call `process::exit` or `panic!` in serve/listen paths, never unconditionally init global tracing, and keep no `OnceLock` singletons that ignore their arguments. Bind and init failures return through the spawn handshake. The workshop listener binds loopback only; only the gateway's own listener may bind wider.

## Feature gating

Gate the leaf, not the call site: a feature cfg's one function body; router composition and `main` stay feature-blind; features forward through Cargo.toml cascades. Never inline cfg-else pairs inside composition expressions.

## Router and module structure

Each feature module exports `fn routes(state) -> Router`. `app.rs` is composition plus `AppState` only. Narrow state per route group with plain `with_state`. A module name states its responsibility; when the name no longer covers what the module owns, rename or split it before adding another responsibility. Use `session.rs` beside `session/`; never introduce `session/mod.rs`. The ceiling ratchet prevents regrowth, not responsibility drift: a server module may not grow past its recorded ceiling, a ceiling is never raised to add a new responsibility, and a split records every new module at its actual size while removing or lowering the old ceiling in the same commit.

## Errors split by boundary

`AppError` is the opaque wire error: one status code per variant, no `#[from]` across the wire boundary, internals leak only in debug builds. Config and spawn errors stay rich and convenient.

## State construction

`AppState` stays typed and construction phased. No service-locator traits, no `Arc<RwLock<Option<T>>>` late-binding slots.

## Server tests

In-process only: `Router::oneshot` or the spawn fixture, with the typed JSON WebSocket client in `tests/common`. Characterization tests pin wire behavior before any session code moves.

## Asset serving and shutdown

No content hashes in asset filenames and no cache headers: the workshop UI is a windowed SPA served from the local process, so nothing is cacheable and the esbuild output keeps its plain names. API-path misses return 404, never the SPA index. The missing-bundle 404 names the build command. Held sockets must never block shutdown: force-exit watchdog plus stopped barrier.

## Transcription boundary

The gateway owns STT through `promptforge-stt`: artifact provisioning, engine construction and teardown, the `/voice` WebSocket, and OpenAI multipart transcription all stay outside this crate. This crate supplies the Workshop listener, status bus, and cross-site guard that the gateway-owned voice routes attach to through `spawn_with_routes`. It never depends on `promptforge-transcribe` or holds whisper model state.
