# promptforge-gateway-local

This crate owns gateway-owned local inference: the artifact store, GGUF
provisioning, dialect probing, the managed `llama-server` child lifecycle,
sidecars, the blob cache store, and CUDA bundle staging.

## Rules

- Local inference provisioning and `llama-server` lifecycle only: no HTTP
  routing, no error envelopes, no profile-switch orchestration. The gateway
  keeps `run_switch`, the `/v1/cache` HTTP adapter, and the routing table.
- The runtime never compiles native dependencies and never invokes CMake,
  NVCC, MSBuild, Git, PowerShell, or any other build tool. Native compilation
  belongs to the Cargo build (`build.rs` plus the `promptforge-gateway-build`
  crate) or to packaging; runtime code may only verify, stage, and launch
  build-produced native bundles.
- The `llama-cuda` feature embeds a build-produced CUDA `llama-server` bundle
  through the generated `llama_cuda_bundle` module. Runtime code consumes the
  embedded manifest and bytes; it never rebuilds or patches them.
- Shared vocabulary comes from below: wire types, `Upstream`, and
  `http_util` from `promptforge-gateway-protocol`; `Model`, `Endpoint`, and
  the dominion queues from `promptforge-gateway-routing`. This crate never
  names gateway concepts (`GatewayError`, `Routing`, profile switching).
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
