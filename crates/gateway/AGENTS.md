# gateway

This crate owns the inference gateway: OpenAI-shaped HTTP routing, profile
switching, and the serving lifecycle. Local model provisioning and the
`llama-server` child lifecycle live in `gateway-local` behind the
default-on `local` feature; the gateway drives them through `LocalRuntime`.
Gateway-hosted speech-to-text lifecycle and HTTP routes live in
`gateway-stt` behind the `workshop` feature.

## Rules

- Runtime and serve paths never compile native dependencies and never
  invoke CMake, NVCC, MSBuild, Git, PowerShell, or any other build tool.
  Native compilation belongs to the `build-llama-cuda` tool on a build
  machine or to packaging; runtime code may only download, verify, stage,
  and launch pinned, checksummed archives.
- The CUDA `llama-server` is a managed download produced by the
  `build-llama-cuda` release workflow, never a Cargo build product.
- The `local` feature is additive and defaults on; `--no-default-features`
  must keep compiling as a headless gateway without the local crate and its
  archive/blocking-HTTP dependencies.
- The `web-search` feature is additive and defaults on; it gates the
  `gateway-web-search` dependency and the
  `POST /v1/tools/web_search` route. The gateway keeps auth and the
  mount/reload shim; the service crate never sees `GatewayError`.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
