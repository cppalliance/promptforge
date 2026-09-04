# gateway-local

This crate owns gateway-owned local inference: the shared artifact store, GGUF provisioning, dialect probing, the managed `llama-server` child lifecycle, sidecars, and the blob cache store.

- Local inference provisioning and `llama-server` lifecycle only: no HTTP routing, no error envelopes, no profile-switch orchestration. The gateway keeps `run_switch`, the `/v1/cache` HTTP adapter, and the routing table. `gateway-stt` reuses the public `ArtifactStore` for speech-model provisioning.
- Shared vocabulary comes from below: wire types, `Upstream`, and `http_util` from `shared-protocol`; `Model`, `Endpoint`, and the dominion queues from `gateway-routing`. This crate never names gateway concepts (`GatewayError`, `Routing`, profile switching).
