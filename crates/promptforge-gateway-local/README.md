# promptforge-gateway-local

Gateway-owned local inference for the PromptForge inference gateway: the
pinned `llama-server` artifact store, GGUF provisioning with digest pins,
dialect probing, the managed `llama-server` child lifecycle with supervised
respawn, HF metadata sidecars, the blob cache store behind the gateway's
`/v1/cache` routes, and CUDA bundle staging.

The gateway drives this crate through `LocalRuntime`: `start` provisions and
launches one child per `[[local_model]]`, `models` yields the routing table
entries, and `shutdown` tears every child down deterministically. The crate
contains no HTTP routing and no profile-switch orchestration; those live in
the gateway.

One feature flag exists:

- `llama-cuda` - on a native Windows x86-64 build with CUDA Toolkit >= 12.8,
  compiles the pinned llama.cpp submodule during the Cargo build and embeds
  the resulting bundle for runtime staging. A no-op on every other target.

Runtime code never compiles native dependencies; it only verifies, stages,
and launches build-produced bundles.
