# gateway-stt-engine

This crate owns the Whisper transcription engine and nothing else: model ownership, stateless interim and final inference workers, silence gating, and the runtime-loaded gateway-whisper-ffi integration.

- Engine-only ownership. This crate never depends on HTTP, WebSocket, or UI crates, and never on `gateway-stt`, `workshop-server`, or the gateway. Gateway-owned artifact provisioning, route state, and activation live in `gateway-stt`.
- Decode jobs are stateless. Guidance, finalized history, segmentation, LocalAgreement state, transcript aggregation, completion, and failure belong to `gateway-stt`; workers retain none of them between jobs and have no reset channel.
- The host configures the engine through `EngineConfig`'s plain values only. Never accept the host's own configuration types: that would be a dependency back on the server.
- Native whisper backends are runtime artifacts. This crate never compiles whisper.cpp or grows platform-backend Cargo features.
- Worker threads own the whisper contexts; callers hand owned sample buffers through channels and await transcripts on oneshots, so blocking inference never touches the tokio executor. Keep it that way.
