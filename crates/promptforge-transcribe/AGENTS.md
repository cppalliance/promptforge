# promptforge-transcribe

This crate owns the Whisper transcription engine and nothing else: model
ownership, the interim and final-pass inference worker threads, energy-based
segmentation, silence gating, and the whisper-rs integration.

## Rules

- Engine-only ownership. This crate never depends on HTTP, WebSocket, or UI
  crates, and never on `promptforge-ws-server`, the gateway, or any other
  PromptForge crate. Session transport, route state, and post-cache
  activation stay in the server.
- The host configures the engine through `EngineConfig`'s plain values only.
  Never accept the host's own configuration types: that would be a dependency
  back on the server.
- The `cuda` feature only forwards to `whisper-rs/cuda`. Features stay
  additive: enabling one may add capability, never remove or rename it.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
- Worker threads own the whisper contexts; callers hand owned sample buffers
  through channels and await transcripts on oneshots, so blocking inference
  never touches the tokio executor. Keep it that way.
