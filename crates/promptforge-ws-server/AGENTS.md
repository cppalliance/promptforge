# Workshop Server Rules

These rules bind `crates/promptforge-ws-server`. The repo-root AGENTS.md applies on top; the embedded UI has its own AGENTS.md under `ui/`. Rules here are target-state: refactor-era code lands in this shape.

## Two-zone error policy

The process has two zones with opposite failure postures. The boundary: config load plus server construction is zone one; request and session handling is zone two.

- Zone one (construction/startup): fail loudly and immediately. Panics, `expect`, and hard errors returned to the host are all correct - a misconfigured process must not limp into serving.
- Zone two (steady state): never panic, and never `unwrap` anything a client sent. Errors are values: error frames, 4xx/5xx responses, status-bus reports, or logged degradation. A lock poisoned by a panicking peer recovers the value rather than wedging the process.

Degrade-not-crash features (voice provisioning, gateway outages) are zone two by definition: absence of a capability is a state, not an error.

## WebSocket session model

One task owns each socket: a single `select!` loop reads and writes the same socket handle. No outbox channel, no writer task, no session registry. Durable messages deliver via `Notify` plus a per-client cursor and coalesce; ephemeral messages go through a bounded broadcast and drop on lag. Malformed inbound frames are logged and skipped, or close the connection with a policy code - never a panic. Shared scaffolding between WebSocket endpoints is deleted, not relocated to a common module.

## Delivery contract

Every pushed message type is classified in the protocol module as durable (delivered exactly via cursor, coalesces) or ephemeral (may drop under lag, fully resent on reconnect). No message type ships unclassified.

## Drop-guard cancellation

Work held on behalf of a client - a gateway completion, a whisper job, a tape span - is wrapped in a guard that cancels on disconnect. A resource that still needs a manual cleanup call is a wrong factoring.

## Embedding and process hygiene

This crate runs inside other binaries. Never call `process::exit` or `panic!` in serve/listen paths, never unconditionally init global tracing, and keep no `OnceLock` singletons that ignore their arguments. Bind and init failures return through the spawn handshake. The workshop listener binds loopback only; only the gateway's own listener may bind wider.

## Feature gating

Gate the leaf, not the call site: a feature cfg's one function body; router composition and `main` stay feature-blind; features forward through Cargo.toml cascades. Never inline cfg-else pairs inside composition expressions.

## Router and module structure

Each feature module exports `fn routes(state) -> Router`. `app.rs` is composition plus `AppState` only. Narrow state per route group with plain `with_state`. Module size ratchet: a server module may not grow past its recorded ceiling (record ceilings when the `routes/` split lands).

## Errors split by boundary

`AppError` is the opaque wire error: one status code per variant, no `#[from]` across the wire boundary, internals leak only in debug builds. Config and spawn errors stay rich and convenient.

## State construction

`AppState` stays typed and construction phased. No service-locator traits, no `Arc<RwLock<Option<T>>>` late-binding slots.

## Server tests

In-process only: `Router::oneshot` or the spawn fixture, with the typed JSON WebSocket client in `tests/common`. Characterization tests pin wire behavior before any session code moves.

## Asset serving and shutdown

No content hashes in asset filenames and no cache headers: the workshop UI is a windowed SPA served from the local process, so nothing is cacheable and the esbuild output keeps its plain names. API-path misses return 404, never the SPA index. The missing-bundle 404 names the build command. Held sockets must never block shutdown: force-exit watchdog plus stopped barrier.

## Transcription boundary

The Whisper engine - model ownership, inference workers, segmentation, silence gating - lives in `promptforge-transcribe`. This crate keeps the voice WebSocket session, route state, the capability probe, startup degradation, and post-cache provisioning and activation. The engine is constructed only through `promptforge_transcribe::EngineConfig`'s plain values, mapped from `VoiceConfig`; never pass `VoiceConfig` itself, and never let the engine crate depend back on this one. GPU transcription is the `voice-cuda` feature (`cuda` remains as a compatibility alias).
