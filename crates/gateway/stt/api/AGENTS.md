# gateway-stt

This crate is the Gateway speech facade for artifact provisioning, engine lifecycle, batch transcription, and Realtime behavior.

- `take::Take` owns all per-take guidance, finalized history, segmentation, hypothesis agreement, transcript aggregation, completion, and failure.
- The facade starts empty and permits exactly one initial engine load, attempted by the Gateway's boot profile command after the listener is serving. No retry, reload, or replacement transition exists; later configuration changes take effect on process restart, and a failed boot load leaves speech unavailable until then.
- Artifact download and verification stay in `gateway-local::artifacts::ArtifactStore`.
- Multipart input is capped at 25 MiB before decode.
