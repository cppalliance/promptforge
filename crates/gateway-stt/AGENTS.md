# gateway-stt

This crate is the gateway speech facade: artifact provisioning, engine lifecycle, batch transcription, and Realtime behavior.

- `take::Take` solely owns per-take guidance, finalized history, segmentation, hypothesis agreement, transcript aggregation, completion, and failure.
- Artifact download and verification stay in `gateway-local::artifacts::ArtifactStore`.
- Speech routes are OpenAI multipart batch transcription and Realtime transcription only. Multipart input is capped at 25 MiB before decode.
