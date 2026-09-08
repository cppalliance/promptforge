# gateway-stt

This crate is the Gateway speech facade for artifact provisioning, engine lifecycle, batch transcription, and Realtime behavior.

- `take::Take` owns all per-take guidance, finalized history, segmentation, hypothesis agreement, transcript aggregation, completion, and failure.
- Artifact download and verification stay in `gateway-local::artifacts::ArtifactStore`.
- Multipart input is capped at 25 MiB before decode.
