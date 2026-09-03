# gateway

This crate owns the inference gateway: OpenAI-shaped HTTP routing, profile switching, and the serving lifecycle.

- The `local` feature is additive and defaults on; `--no-default-features` must keep compiling as a headless gateway without the local crate and its archive/blocking-HTTP dependencies. Local model provisioning and the `llama-server` child lifecycle live in `gateway-local` behind that feature; the gateway drives them through `LocalRuntime`.
- The CUDA `llama-server` is a managed download produced by the `build-llama-cuda` release workflow, never a Cargo build product.
- The `web-search` feature is additive and defaults on; it gates the `gateway-web-search` dependency and the `POST /v1/tools/web_search` route. The gateway keeps auth and the mount/reload shim; the service crate never sees `GatewayError`.
- Gateway-hosted speech-to-text lifecycle and HTTP routes live in `gateway-stt` behind the `workshop` feature.
