# gateway

This crate owns the inference gateway: OpenAI-shaped HTTP routing, profile switching, and the serving lifecycle.

- The `local` feature is additive and defaults on; `--no-default-features` must keep compiling as a headless gateway without the local crate and its archive/blocking-HTTP dependencies. Local model provisioning and the `llama-server` child lifecycle live in `gateway-local` behind that feature; the gateway drives them through `LocalRuntime`.
- The CUDA `llama-server` is a managed download produced by the `build-llama-cuda` release workflow, never a Cargo build product.
- The `web-search` feature is additive and defaults on; it gates the `gateway-web-search` dependency and the `POST /v1/tools/web_search` route. The gateway keeps auth and the mount/reload shim; the service crate never sees `GatewayError`.
- Gateway-hosted speech-to-text lifecycle and HTTP routes live in `gateway-stt` behind the default-on `stt` feature; a `--no-default-features` build stubs the route and refuses `[[stt_model]]` configurations.
- The gateway never hosts or embeds the workshop: the desktop shell spawns `workshop-server` in-process and attaches over HTTP, and the `gateway` crate has no `workshop` feature and no `workshop-server` dependency (the `gateway-stt` crate keeps its own `workshop-server` edge for the `/stt` socket attach API until voice migrates into workshop-server). A boot config carrying a `[workshop]` section must keep parsing - startup logs a deprecation warning naming the inert `bind`/`open_browser` fields and the still-live `[workshop.stt]` capture tuning; never fail or silently ignore it.
