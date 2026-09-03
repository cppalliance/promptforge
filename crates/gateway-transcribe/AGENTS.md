# gateway-transcribe

This crate owns the Whisper transcription engine and nothing else: model
ownership, the interim and final-pass inference worker threads, energy-based
segmentation, silence gating, and the runtime-loaded gateway-whisper-ffi integration.

## Rules

- Engine-only ownership. This crate never depends on HTTP, WebSocket, or UI
  crates, and never on `gateway-stt`, `promptforge-workshop-server`, or the
  gateway. Gateway-owned artifact provisioning, route state, and activation
  live in `gateway-stt`.
- The host configures the engine through `EngineConfig`'s plain values only.
  Never accept the host's own configuration types: that would be a dependency
  back on the server.
- Native whisper backends are runtime artifacts. This crate never compiles
  whisper.cpp or grows platform-backend Cargo features.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
- Worker threads own the whisper contexts; callers hand owned sample buffers
  through channels and await transcripts on oneshots, so blocking inference
  never touches the tokio executor. Keep it that way.
