# gateway-stt

This crate is the gateway speech facade: artifact provisioning, engine lifecycle, batch transcription, and Realtime behavior.

- `take::Take` solely owns per-take guidance, finalized history, segmentation, LocalAgreement state, transcript aggregation, completion, and failure.
- Artifact download and verification stay in `gateway-local::artifacts::ArtifactStore`.
- `/stt` keeps its existing wire path and frame contract. OpenAI multipart input is capped at 25 MiB before decode.
