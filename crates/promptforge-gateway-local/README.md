# promptforge-gateway-local

Gateway-owned local inference for the PromptForge inference gateway: the
pinned `llama-server` artifact store, GGUF provisioning with digest pins,
dialect probing, the managed `llama-server` child lifecycle with supervised
respawn, HF metadata sidecars, the blob cache store behind the gateway's
`/v1/cache` routes, the orphan scan behind the gateway's `GET /admin/orphans`
route (files under the cache's `models/` tree no loaded `[[local_model]]`
entry references), the bounded GGUF header parser behind the gateway's
`GET /admin/model-info` route (architecture, layer count, parameter count,
and optional `tokenizer.chat_template` - never tensor data), the twelve-family
bundled chat-template catalog with hash-first known overrides, and CUDA bundle
staging.

Chat launches keep `--jinja` enabled and resolve templates in this order:
an explicit custom file, an explicit `builtin:<family>` asset staged under
`chat-templates/`, a known broken-template override selected by embedded hash
then sidecar model ID, or the GGUF's non-empty embedded template. A chat model
without any usable template is refused with the supported fixes.
`inspect_chat_template` exposes that same decision without staging an asset,
so the gateway can report effective sources and reasons to the Config UI.

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
