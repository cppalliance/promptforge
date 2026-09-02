# promptforge-stt

This crate owns gateway-hosted speech-to-text runtime behavior: artifact
provisioning, active-profile engine lifecycle, the `/stt` WebSocket, and the
OpenAI-compatible transcription endpoint.

## Rules

- Runtime ownership only. Whisper inference primitives stay in
  `promptforge-transcribe`; artifact download and verification stay in
  `promptforge-gateway-local::artifacts::ArtifactStore`.
- The gateway selects profiles and supplies validated config. This crate
  provisions only the selected `Config::stt_models()` pair.
- The whisper.cpp runtime is provisioned through `ArtifactStore` and handed
  to `promptforge-transcribe` as a path. Native backends are never Cargo
  features.
- `/stt` keeps its existing wire path and frame contract. OpenAI multipart
  input is capped at 25 MiB before decode.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
