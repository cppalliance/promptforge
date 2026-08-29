# promptforge-gateway

This crate owns the inference gateway: OpenAI-shaped HTTP routing, profile
switching, and the serving lifecycle. Local model provisioning and the
`llama-server` child lifecycle live in `promptforge-gateway-local` behind the
default-on `local` feature; the gateway drives them through `LocalRuntime`.

## Rules

- Runtime and serve paths never compile native dependencies and never
  invoke CMake, NVCC, MSBuild, Git, PowerShell, or any other build tool.
  Native compilation belongs to the Cargo build (the local crate's
  `build.rs` plus the `promptforge-gateway-build` crate) or to packaging;
  runtime code may only verify, stage, and launch build-produced native
  bundles.
- The `llama-cuda` feature forwards to `promptforge-gateway-local`, which
  embeds the build-produced CUDA `llama-server` bundle through the generated
  `llama_cuda_bundle` module. Runtime code consumes the embedded manifest
  and bytes; it never rebuilds or patches them.
- Build-time logic lives in `promptforge-gateway-build`, not in the gateway
  library, so the runtime crate carries no build-tool code paths.
- The `local` feature is additive and defaults on; `--no-default-features`
  must keep compiling as a headless gateway without the local crate and its
  archive/blocking-HTTP dependencies.
- The `web-search` feature is additive and defaults on; it gates the
  `promptforge-web-search-service` dependency and the
  `POST /v1/tools/web_search` route. The gateway keeps auth and the
  mount/reload shim; the service crate never sees `GatewayError`.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
