# gateway-local

This crate owns gateway-owned local inference: the shared artifact store, GGUF
provisioning, dialect probing, the managed `llama-server` child lifecycle,
sidecars, and the blob cache store. `gateway-stt`
reuses the public `ArtifactStore` for speech-model provisioning.

## Rules

- Local inference provisioning and `llama-server` lifecycle only: no HTTP
  routing, no error envelopes, no profile-switch orchestration. The gateway
  keeps `run_switch`, the `/v1/cache` HTTP adapter, and the routing table.
- The runtime never compiles native dependencies and never invokes CMake,
  NVCC, MSBuild, Git, PowerShell, or any other build tool. Native compilation
  belongs to the `llama-cuda-build` tool running on a build machine or to
  packaging; runtime code may only download, verify, stage, and launch
  pinned, checksummed archives.
- Shared vocabulary comes from below: wire types, `Upstream`, and
  `http_util` from `gateway-protocol`; `Model`, `Endpoint`, and
  the dominion queues from `gateway-routing`. This crate never
  names gateway concepts (`GatewayError`, `Routing`, profile switching).
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
