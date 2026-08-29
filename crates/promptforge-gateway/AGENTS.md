# promptforge-gateway

This crate owns the inference gateway: OpenAI-shaped HTTP routing, local
model provisioning, and the `llama-server` child lifecycle.

## Rules

- Runtime and serve paths never compile native dependencies and never
  invoke CMake, NVCC, MSBuild, Git, PowerShell, or any other build tool.
  Native compilation belongs to the Cargo build (`build.rs` plus the
  `promptforge-gateway-build` crate) or to packaging; runtime code may only
  verify, stage, and launch build-produced native bundles.
- The `llama-cuda` feature embeds a build-produced CUDA `llama-server`
  bundle through the generated `llama_cuda_bundle` module. Runtime code
  consumes the embedded manifest and bytes; it never rebuilds or patches
  them.
- Build-time logic lives in `promptforge-gateway-build`, not in the gateway
  library, so the runtime crate carries no build-tool code paths.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
