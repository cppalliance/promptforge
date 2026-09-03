<!-- source: promptforge codebase analysis (gateway, gateway-protocol, gateway-config, gateway-local, gateway-stt crates as of 2026-09-03), stable-diffusion.cpp master source (examples/server, August 2026), and five-angle existing-practice research (OpenAI Images API, sd-server, gateway prior art, local serving practice, Rust practice) -->
# Image Generation Endpoint for the PromptForge Gateway: Implementation Specification

**Report Type**: Analytical / recommendation report (implementation specification variant), prepared for a Rust implementor working with LLM coding assistance.

## Executive Summary

Add OpenAI-shaped image generation to the gateway as `POST /v1/images/generations`, in two phases. Phase 1 adds a fourth `ModelKind` (`Image`), request/response wire types, an `Upstream::send_images` trait method that declines by default, an `OpenAiUpstream` passthrough, and a gateway route that mirrors the existing rerank handler line for line. Phase 1 ships remote image generation (OpenAI, Together, and any OpenAI-compatible provider) with no local-inference changes and is low risk (high confidence: it replicates a pattern the codebase already runs three times). Phase 2 adds local open-weight generation through a managed `sd-server` child process from stable-diffusion.cpp, whose server mode natively speaks the OpenAI images API. Phase 2 follows the structural precedent speech-to-text set: a dedicated `[[image_model]]` config table, a dedicated crate owning the child lifecycle, and a runtime wired into the profile-switch path.

This specification amends one standing design decision. The gateway comparison report (`report-promptforge-gateway-comparison.md`, section 3.6) assigns local image generation to an external ComfyUI WebSocket bridge. This document recommends a gateway-supervised `sd-server` child instead, on the grounds that the comparison report's own first criterion - hardware and VRAM lifecycle coordination - cannot be enforced against a Python daemon the gateway does not own. Section 9 states the trade-off in full.

The principal risks are operational rather than architectural, and each is now evidence-backed: the shared 120-second whole-request timeout kills legitimate long generations and needs a per-route read-timeout policy; the 4 MiB response-body ceiling (`MAX_JSON_BODY`) is too small for base64 image payloads; and sd-server supervision differs from llama-server's in three source-verified ways - the child has no auth flag, readiness is signaled by the port bind rather than a health endpoint, and an unchecked `listen()` can exit 0 on a port-bind failure.

## Contents

1. Recommendation
2. How the gateway dispatches workloads today
3. The endpoint contract
4. Phase 1: remote-capable endpoint, crate by crate
5. Phase 2: local open-weight generation via sd-server
6. Decision points the implementor must resolve
7. Test plan
8. Documentation and convention obligations
9. Alternatives considered and set aside
10. Risks and limitations

## 1. Recommendation

Build the feature in two phases, landing Phase 1 first as its own commit series. Phase 1 is a pure passthrough: it touches `gateway-config`, `gateway-protocol`, and `gateway`, and every change has an in-repo template (the rerank workload) to copy. Phase 2 is a new local runtime: it touches `gateway-config`, a new `gateway-image` crate, and the gateway's profile-switch orchestration, and it reuses the public `ArtifactStore` the way `gateway-stt` does. Do not start Phase 2 by modifying `gateway-local`'s `LaunchOptions`; that structure is a rendered `llama-server` argv and does not fit a diffusion server (medium confidence: see section 5 for the argument and section 9 for the rejected alternative).

## 2. How the gateway dispatches workloads today

The gateway classifies every configured model by kind and pairs each route with a kind guard. `ModelKind` today has exactly three variants, `Chat`, `Embedding`, and `Classifier` (`gateway-config/src/config.rs:527-535`). Each inference route in `gateway/src/lib.rs` runs the same seven steps in order: bearer auth, request-shape validation, in-flight registration for profile-switch drain, model lookup, kind guard, dominion queue admission, and upstream forwarding followed by response validation. The rerank handler is the cleanest template because it has no streaming branch:

```rust
async fn rerank(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RerankRequest>,
) -> Result<Json<RerankResponse>, GatewayError> {
    check_auth(&state, &headers).await?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let in_flight = state.begin_inference().await;
    let model = {
        let live = state.live.read().await;
        live.routing.model(&request.model)?
    };
    crate::routing::require_kind(&model, ModelKind::Classifier)?;
    let client_id = crate::queue::ClientId::from_header(
        headers
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let _permit = tokio::select! {
        result = model.endpoint.queue.admit(client_id.as_str()) => result?,
        () = in_flight.cancelled() => return Err(GatewayError::RequestCancelled),
    };
    let response = tokio::select! {
        result = model.endpoint.upstream.send_rerank(request, &model.upstream_name) => result?,
        () = in_flight.cancelled() => return Err(GatewayError::RequestCancelled),
    };
    response
        .validate()
        .map_err(|reason| GatewayError::upstream_protocol(std::io::Error::other(reason)))?;
    Ok(Json(response))
}
```

Three seams carry the workload abstraction, and the new feature extends each exactly once. The `Upstream` trait in `gateway-protocol/src/upstream.rs:44-148` declares one method per workload, and every non-chat method ships a default body that returns `ProtocolError::ModelUnavailable`, so upstreams that cannot serve a workload decline it rather than fabricate a response. The kind guard `require_kind` (`gateway/src/routing.rs:171-181`) rejects a model whose configured kind differs from the route's workload with `GatewayError::KindMismatch`. And the wire types in `gateway-protocol/src/wire.rs` follow one recipe: named fields the gateway routes on, a `#[serde(flatten)] rest: Map<String, Value>` that preserves everything else verbatim, a `RESERVED` key list, and a `validate()` that rejects empty required fields and reserved-key smuggling (see `RerankRequest`, `wire.rs:341-381`).

Two cross-cutting behaviors apply to every new inference route and are easy to miss. First, model-name rewriting (invariant UP-008): the upstream substitutes the configured `upstream_name` into the outgoing body and restores the caller's model name on the response, so the backend never sees the caller-facing name and vice versa (`gateway-protocol/src/upstream.rs:343-355`). Second, drain integration: every inference handler registers with `state.begin_inference()` and wraps both queue admission and the upstream call in `tokio::select!` against `in_flight.cancelled()`, so a profile switch can drain or cancel in-flight work (`gateway/src/lib.rs:465-479`).

## 3. The endpoint contract

Adopt the OpenAI Images API shape at `POST /v1/images/generations`, with `b64_json` as the response format the gateway guarantees. The OpenAI shape is the right contract for three reasons: existing OpenAI client libraries can drive the gateway without modification; stable-diffusion.cpp's `sd-server` already implements an OpenAI-compatible images endpoint, so Phase 2 needs no translation layer (verified against the project's server source, August 2026); and the gateway's stated purpose is OpenAI-shaped passthrough, so a bespoke shape would break the crate's own charter. The `url` response format passes through for remote providers that offer it, but local generation returns `b64_json` only, because the gateway has no blob-serving lifecycle for ephemeral images.

The request type names exactly five fields: `model` and `prompt`, plus optional `n`, `size`, and `quality`. Everything else rides in `rest`, and two formerly obvious candidates belong there deliberately: `style` and `response_format` are DALL-E-only legacy fields that GPT image models reject with a 400, so naming them would advertise a capability the modern models refuse. The same `rest` channel carries the GPT-image knobs (`background`, `moderation`, `output_format`, `output_compression`) and provider or engine extras (`seed`, `steps`, `cfg_scale`). The response type names `created` and `data`, where each entry carries optional `b64_json`, `url`, and `revised_prompt`; GPT image models also return a `usage` object with token breakdowns and echo `background`, `output_format`, `quality`, and `size` at the top level, all of which ride in `rest`. Unlike chat, embeddings, and rerank, the OpenAI images response has no `model` field (confirmed against the Stainless-generated SDK types), so the rewrite-back half of UP-008 is a no-op here; the implementor should note that asymmetry in the method's doc comment rather than invent a field.

**Streaming and edits are named future phases, not silent omissions.** OpenAI image streaming (`stream: true` with `partial_images`) uses SSE with exactly two flat event types, `image_generation.partial_image` and `image_generation.completed`; it is deferred to a Phase 3 because the gateway's SSE relay is typed on chat chunks, and the right shape is a small dedicated event enum rather than a shoehorn. `POST /v1/images/edits` (multipart, up to 16 images) is likewise a named follow-up; `/v1/images/variations` is DALL-E-2-only legacy and is skipped entirely.

## 4. Phase 1: remote-capable endpoint, crate by crate

Phase 1 lands four coordinated changes. Each names its template, and the implementor should diff each new item against its template before committing.

**`gateway-config`: add the `Image` kind.** Add `Image` to `ModelKind` (`config.rs:527-535`) with the serde spelling `"image"` and the `Display` arm `"image"`. The enum is `#[non_exhaustive]`, so downstream `match` statements with a wildcard arm keep compiling; the two that matter are the launch-mode mapping in `gateway-local/src/runtime.rs:549-554` (which already routes unknown kinds to `ServeMode::Chat` and must stay unreachable for image models because Phase 1 declares no local image models) and the kind-scope validator `validate_kind_scope` (`gateway-config/src/config/validate.rs:714-734`), which automatically rejects chat-only fields (`thinking`, `effort_levels`, `default_effort`, `adaptive_thinking`) for the new kind because it gates on `kind == ModelKind::Chat`. Verify that `[[model]]` validation passes the chat-only extras (`tool_dialect`, and any chat-scoped fields) into `validate_kind_scope`'s `extra` argument for the new kind; read the call sites before assuming.

**`gateway-protocol`: wire types and the trait method.** Add `ImageGenerationRequest` and `ImageGenerationResponse` to `wire.rs` beside `RerankRequest`, following the recipe in section 2: named fields, `rest` passthrough, `RESERVED` list (`model`, `prompt`, `n`, `size`, `quality`), and a `validate()` returning static reason strings. Add `send_images` to the `Upstream` trait with a default body returning `ProtocolError::ModelUnavailable(req.model)`, copying the doc-comment shape of `send_rerank` (`upstream.rs:80-99`). Implement it on `OpenAiUpstream` by cloning the `send_rerank` body (`upstream.rs:371-384`) and changing the path to `"images/generations"`. The crate's AGENTS.md forbids naming gateway-local concepts here; the change needs none.

**Timeout and payload policy change with the payload.** The shared HTTP policy (`gateway-protocol/src/http_util.rs`) applies a 120-second whole-request `REQUEST_TIMEOUT`, and that is a total deadline from connect to last body byte: field evidence from production image proxies shows total deadlines kill legitimate long generations, which produce thirty to two hundred fifty seconds of silence followed by a multi-megabyte burst (NVIDIA's OpenShell hit exactly this failure on streaming inference). The images path needs a different policy: the connect timeout stays at 10 seconds, the whole-request deadline is replaced by reqwest's `read_timeout` sized to worst-case generation (120 seconds default; 300 to 600 seconds for low-VRAM offload profiles), and dominion queue wait never counts against the generation window because admission completes before the upstream call starts. On the payload itself, base64 image data stays `String` or `serde_json::value::RawValue` end to end: decoding into a `Vec<u8>` serde field re-serializes as an array of integers, a fourfold expansion tracked in a known serde_json issue. Validation sniffs PNG magic bytes only; the gateway never re-encodes.

**`gateway`: the route and handler.** Add an `image_generations` handler in `gateway/src/lib.rs` that reproduces the rerank handler's seven steps with `ModelKind::Image` and `send_images`, and mount it in `build_router` beside the other unconditional `/v1` routes (`lib.rs:290-299`). The route is unconditional, not feature-gated: like embeddings and rerank, it is a plain passthrough that needs no local runtime. No new `GatewayError` variant is needed; `KindMismatch`, `UnknownModel`, queue errors, and the `Protocol` wrapper cover every failure path, and the classify table (`gateway/src/error.rs:294-433`) already maps them. Update the crate-level doc comment's "What ships" paragraph (`lib.rs:1-57`), which enumerates every route.

**Catalog surfacing.** `GET /v1/models` serializes `ModelInfo.kind` verbatim (`wire.rs:450-467`, `gateway/src/lib.rs:686-709`), so image models appear with `"kind": "image"` and no handler change. One collision needs care: `Capabilities.images` is a boolean meaning "accepts image inputs" (vision), not "generates images". Do not reuse it; the kind field is the discriminator, and the doc comment on `Capabilities::images` should gain a sentence saying so.

**Validation and error mapping follow field-tested prior art.** Four practices from production gateways apply directly. First, per-model validation tables - allowed sizes, the `n` range, and maximum prompt length, the three-map pattern one-api uses - are enforced before queue admission, so a bad request fails with a 400 and never burns a queue slot. Second, upstream error codes pass through verbatim and classification keys on the structured `code`, never on message strings: `content_policy_violation` and `moderation_blocked` are 400s the client must not retry, `rate_limit_exceeded` is a 429 with its `Retry-After` preserved, and LiteLLM's regression (issue #19328, where string-matching collapsed Azure's structured content-policy response into a generic 400) is the cautionary tale. Third, the parameter policy is accept-and-ignore for cosmetic knobs (`style`, `user`) but a hard 400 for capabilities the route cannot honor; silently dropping a parameter is the one unacceptable outcome. Fourth, the gateway records upstream request ids and never blindly retries: upstreams finish and bill after a client disconnect, so a retry can double-bill.

## 5. Phase 2: local open-weight generation via sd-server

Phase 2 gives the operator all-you-can-eat local generation from open weights (FLUX.1, SDXL, SD 3.5, Qwen-Image, Z-Image, and the other pipelines stable-diffusion.cpp supports) at zero marginal cost. The design copies what speech-to-text just did, because STT is the codebase's most recent answer to the same question: how to add a non-chat modality with its own model artifacts and runtime.

**The STT precedent is the structural template.** STT ships as a dedicated config table `[[stt_model]]` (`gateway-config/src/config.rs:201-202`), a dedicated crate `gateway-stt` owning provisioning and inference, a runtime (`SttRuntime`) constructed and torn down inside `replace_runtimes` alongside the local runtime (`gateway/src/lib.rs:1147-1252`), and a route that reuses the shared auth and drain machinery (`lib.rs:421-435`). `gateway-stt` provisions its model files through `gateway-local`'s public `ArtifactStore` without depending on any llama-specific machinery, which the `gateway-local` AGENTS.md explicitly sanctions ("`gateway-stt` reuses the public `ArtifactStore` for speech-model provisioning").

**Apply the same shape to images.** Concretely: a new `[[image_model]]` table in `gateway-config` carrying `name`, `description`, `source` (the diffusion model weights), optional `sha256`, optional `dominion`, `vram_gb`, and a companion block for the text encoders and VAE; a new `gateway-image` crate owning sd-server provisioning, child spawn, readiness, respawn, and an `Upstream` implementation whose `send_images` forwards to the child's loopback OpenAI endpoint; and wiring in `replace_runtimes` so image children stop before and start after the VRAM-owning chat children, preserving the invariant that old and new VRAM owners never coexist (`gateway/src/lib.rs:1002-1044`). Companion weights map onto the existing companion-provisioning pattern: each companion resolves through `ArtifactStore::ensure_model` under its own source identity and pin, and any failure returns before the child spawns (`gateway-local/src/runtime.rs:584-612`).

**The child-supervision code needs generalization, not reuse.** `ServerGuard` and `LaunchOptions` in `gateway-local/src/server.rs` are llama-shaped: `LaunchOptions` is a typed `llama-server` argv (`--ctx-size`, `--n-predict`, `--flash-attn`, and the `ServeMode` flag at `server.rs:129-136`), and readiness polls llama's health endpoint with a per-attempt bearer key. An sd-server child differs on both axes, and every difference below is verified against the project's master source (`examples/server/runtime.cpp`, `main.cpp`, `routes_openai.cpp`, `async_jobs.cpp`, August 2026) rather than inferred from documentation. The portable core is the process lifecycle: free-port allocation, spawn with captured output, bounded readiness, redacted debug rendering, respawn with cooldown, and bounded teardown. The recommended move is to lift that core into a shared form (either a public module in `gateway-local` or a small new crate both `gateway-local` and `gateway-image` depend on) and leave argv rendering and readiness per engine (medium confidence: the cleanest extraction boundary only becomes obvious once the implementor reads `server/support.rs` in full; the fallback of duplicating the supervisor inside `gateway-image` is acceptable and keeps `gateway-local` untouched).

**sd-server has no authentication, so the gateway remains the whole auth boundary.** Unlike llama-server, sd-server's server parameters contain no `--api-key` or any auth flag (verified in `SDSvrParams::get_options()`); its only pre-routing middleware sets permissive CORS headers. The per-attempt loopback credential pattern from llama-server does not transfer. The image child binds loopback only (the default; never pass `--listen-ip 0.0.0.0`), the gateway authenticates callers as it does today, and requests forward to the child with no Authorization header at all.

**Readiness is the port bind, and exit codes lie.** sd-server loads its full model context (`new_sd_ctx()`) before calling `listen()`, so a bound port implies loaded weights: the readiness probe is a TCP connect or a `GET /v1/models` poll, with a 60 to 120 second deadline for FLUX-scale cold loads. Two failure modes need explicit handling. A context-load failure exits 1 after logging `new_sd_ctx_t failed`. A port-bind failure is worse: `listen()`'s return value is never checked, so the process logs "listening on" and exits 0. The supervisor must treat any child exit before the first successful probe as a startup failure and disambiguate through the captured output, never through the exit code.

**One child is one generation slot.** Every generation, synchronous or async, holds a single context mutex for its full duration, and the project's author states that parallel generation on one context is slower than sequential. The OpenAI-compatible endpoint is synchronous and simply blocks on that mutex, so concurrent requests serialize with connections held open. The gateway therefore admits one in-flight generation per image child and fails overflow fast through the existing queue errors. The child's OpenAI surface accepts `prompt`, `n`, `size`, `output_format`, and `output_compression`, silently ignores every other OpenAI field, and always returns `b64_json` - behavior that matches the accept-and-ignore parameter policy in section 4. The native `/sdcpp/v1` async API (64-deep FIFO, 429 on overflow, `queue_position` in job status, queued-job cancellation, completed-job results expiring after 600 seconds) is the documented path if deeper queueing or queue-position reporting is ever wanted. Client disconnect cancels the in-flight generation (added upstream in April 2026), which composes cleanly with the gateway's drop-all-the-way-down cancellation chain.

**Pin the binary like the model weights.** sd-server ships rolling Windows CUDA prebuilts tagged `master-<N>-<sha>` with per-asset SHA-256 digests; provisioning pins a tag and matches assets by pattern (`*-bin-win-cuda12-x64.zip` plus the `cudart` DLL zip), never by hardcoded name, because asset naming drifts between rolling tags. Always pass `--lora-model-dir` explicitly: the default `.` makes `/sdcpp/v1/capabilities` return 500 when the working directory is not usable (upstream issue #1468).

**Queue admission and VRAM accounting carry over unchanged.** A `[[image_model]]` bound to a `[[dominion]]` admits through that dominion's shared queue exactly like a local chat model (`gateway-local/src/runtime.rs:519-537`), and its `vram_gb` feeds the same co-residency check. Because a diffusion child holds VRAM for the profile's lifetime, the dominion's VRAM budget is what prevents an image model and a chat model from over-committing one GPU; the implementor should verify the co-residency check reads `[[image_model]]` entries, not only `[[local_model]]`.

**Budget VRAM from server measurements, not CLI measurements.** sd-server holds one to two GiB more peak VRAM than sd-cli for the same model, because the server cannot drop text-encoder weights after conditioning or DiT weights before VAE decode (upstream issue #1293). Table 1 gives starting `vram_gb` budgets from measured stable-diffusion.cpp data, with companion overhead included where noted; `--offload-to-cpu` collapses the budget to an activation floor of roughly 3 to 4 GB at the cost of about 32 GB of system RAM.

| Model and quantization | `vram_gb` starting budget | Notes |
| --- | --- | --- |
| FLUX.1 (schnell or dev) Q4_K | 8 | DiT 6.4 GB plus ~2 GB compute buffer with flash attention; assumes T5-XXL and VAE on CPU |
| FLUX.1 Q8_0 | 15 | DiT 12.1 GB plus compute buffer |
| FLUX.1 bf16 | 24 | Full pipeline resident; do not co-resident with an LLM |
| SDXL fp16 | 9 | Single-file checkpoint |
| SD 3.5 Medium | 7-11 | 7 with T5 offloaded, 11 with all encoders resident |
| SD 3.5 Large | 12-16 | FP8 to FP16 |
| Qwen-Image Q4_K_M | 14 | Qwen2.5-VL encoder offloaded |
| Z-Image-Turbo Q4 | 7 | Qwen3-4B encoder as GGUF |

Table 1: Starting `vram_gb` budgets per image model, from stable-diffusion.cpp's measured data and secondary benchmarks. Confidence is high for the FLUX rows (first-party documentation) and medium for the Qwen-Image and Z-Image rows (secondary sources).

**Readiness ends with a warm-up generation, not a port bind.** The first generation after spawn allocates compute buffers and triggers backend autotune, a multi-second one-time cost the readiness probe does not exercise. After the port binds, the supervisor runs one tiny generation (256x256, one to four steps) and marks the child ready only when it completes; profile-switch cost is therefore load time plus warm-up, on the order of tens of seconds on consumer hardware.

**Provisioning pins provenance, and provenance is load-bearing.** FLUX GGUFs must come from leejet's own Hugging Face repos: the widely mirrored city96 GGUFs are built for ComfyUI's GGUF node and fail in sd-server with the generic `new_sd_ctx_t failed`, a documented footgun that has wasted operators' hours. Companion files (the `ae.safetensors` VAE, `clip_l`, `t5xxl`, or the Qwen encoders) remain first-class pinned assets under the companion pattern. Licensing differs by pipeline and belongs in the operator-facing docs: FLUX.1-schnell, Qwen-Image, and Z-Image-Turbo are Apache 2.0, while FLUX.1-dev is non-commercial-gated and needs an HF token to download.

**Progress reporting resolves with evidence.** sd-server exposes no per-step progress: job status carries `queue_position` and a binary `generating` state with timestamps, and the server never installs the library's progress callback. Phase 2 progress is therefore queue position plus an elapsed-time estimate against the model's known seconds-per-step; a true sampling bar would require parsing `-v` stderr or patching the server, and is recorded here as a known gap rather than promised.

**Implementation notes for the new crate.** The existing `ServerGuard` uses `std::process` with supervisor threads; field practice in tokio services prefers `tokio::process` with `select!` over wait-versus-shutdown, and reliable Windows tree-kill uses a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (the `command-group` crate is the battle-tested wrapper). `gateway-image` should follow that guidance for its own child; this is guidance for the new crate, not a demand to rewrite `gateway-local`.

## 6. Decision points the implementor must resolve

Table 2 lists the decisions this specification leaves open, with the recommended resolution for each. Resolve them before writing code; each is cheap now and expensive after.

| Decision | Options | Recommendation | Confidence |
| --- | --- | --- | --- |
| Config spelling for the kind | `image`, `image-generation` | `image`, matching the one-word spellings of the existing kinds | High: consistency with `chat`/`embedding`/`classifier` |
| `context` on image models | Keep required with a nominal value; make it optional for non-chat kinds | Make it optional for `kind = "image"`; a diffusion model has no token context, and a nominal number would lie on the catalog | Medium: verify whether `ModelConfig.context` is load-bearing outside chat before changing the schema |
| Response body ceiling | Raise `MAX_JSON_BODY`; add a dedicated larger cap for images | A dedicated cap of 64 MiB on the images path; the shared 4 MiB ceiling (`gateway-protocol/src/http_util.rs:14`) fits chat, but field data shows 1024x1024 medium PNG at ~1.9 MB base64, 1536x1024 at ~2.6 MB, and high quality up to ~6 MB per image, so 64 MiB covers n=10 with headroom | High: measured payload data, not preference |
| Phase 2 crate layout | Extend `gateway-local`; new `gateway-image` crate | New `gateway-image` crate, per the STT precedent and the `gateway-local` charter, which scopes that crate to the `llama-server` lifecycle | High: both AGENTS.md charters point this way |
| Upstream timeout policy | Keep the shared 120 s whole-request deadline; per-route read timeout | Connect timeout plus reqwest `read_timeout` sized to worst-case generation (120 s default, 300-600 s low-VRAM), with queue wait excluded from the generation window | High: production incident evidence (NVIDIA OpenShell) and proxy operational docs |
| Progress reporting for generation steps | None; ProgressHub leaves per request | Queue position plus an elapsed-time estimate; sd-server exposes no per-step signal, so a true sampling bar would require parsing `-v` stderr or patching the server - recorded as a known gap | High: verified against the server's async job source |

Table 2: Open decisions, with recommendations. Confidence reflects evidence strength in the current tree.

## 7. Test plan

Every test below has a running example in the tree; copy the harness, not just the assertion.

* **Wire validation** (`gateway-protocol/src/wire.rs` test module): empty model, empty prompt, reserved-key smuggling into `rest`, and a response missing `data`, mirroring the `RerankRequest` tests.
* **Upstream forwarding** (`gateway-protocol/src/upstream.rs` tests): the `serve_once` mock-backend harness (`upstream.rs:412-439`) captures the raw request; assert the path is `POST /images/generations`, the body carries the upstream model name, and the caller's name never leaks (the UP-008 assertions at `upstream.rs:460-482`). Add a default-decline test proving an upstream without an images implementation returns `ModelUnavailable`, mirroring `default_send_rerank_is_model_unavailable` (`upstream.rs:576-602`).
* **Route behavior** (`gateway` tests): auth before body extraction, unknown model returns the `model_not_found` envelope, and a chat model named on the images route returns `kind_mismatch`, following the router-level `oneshot` pattern in `transcription_auth_tests` (`gateway/src/lib.rs:1487-1568`).
* **Config validation** (`gateway-config` tests): `kind = "image"` parses and displays as `image`; chat-only fields on an image model are rejected; the catalog round-trips the new kind.
* **Phase 2 launch rendering** (`gateway-image` tests): the sd-server argv renders the configured model, companions, port, and listen address, with the credential redaction pattern from `RedactedArgs` (`gateway-local/src/server.rs:84-100`) applied to any secret-bearing argument.

Run `cargo test` per touched crate, then the workspace, then clippy with the workspace's lint configuration. Some crates enforce per-module line ceilings through `module-ceilings.toml` files; check for one in each touched crate and stay under it.

## 8. Documentation and convention obligations

The codebase treats doc comments and crate charters as load-bearing, and the change is incomplete without them. Update the "What ships" paragraph in `gateway/src/lib.rs:9-57`, the gateway README's route table, and the user guide. Every new public item needs a doc comment in the house style: first sentence states what it is, `# Errors` sections name each failure variant, and invariants carry requirement IDs (WIRE-, UP-, PFGL-) where a new invariant is introduced; do not mint IDs for behavior that merely inherits an existing one. New config types use `#[serde(deny_unknown_fields)]` and `#[non_exhaustive]`, matching `LocalModelConfig`. The `gateway` and `gateway-protocol` AGENTS.md boundaries in section 4 are hard constraints: no gateway-local concepts in `gateway-protocol`, no HTTP routing in `gateway-local` or the new `gateway-image`.

## 9. Alternatives considered and set aside

Four credible alternatives were weighed. The first is a remote-only feature that skips Phase 2: it costs less, but the stated purpose of the feature is all-you-can-eat local generation from open weights, so skipping Phase 2 fails the requirement rather than meeting it. The second is extending `[[local_model]]` and `gateway-local` with a second engine instead of a new table and crate: this avoids a new crate, but it forces `LaunchOptions` to carry two disjoint argv schemas and makes `gateway-local` own two engine lifecycles, against its written charter; the STT precedent shows the codebase absorbs new modalities as separate tables and crates, and consistency with that pattern lowers review cost. The third is per-request CLI invocation of `sd-cli` instead of a persistent `sd-server`: it needs no supervisor, but it reloads multi-gigabyte weights on every request, turning a seconds-scale generation into a minutes-scale one, and it discards the queue, drain, and respawn machinery the gateway already trusts.

The fourth alternative is the standing design: an external ComfyUI WebSocket bridge, as specified in `report-promptforge-gateway-comparison.md` section 3.6. ComfyUI offers unmatched workflow flexibility and the fastest community support for new diffusion architectures, and the bridge keeps multi-gigabyte Python dependencies out of the gateway's supply chain. It is set aside because it delegates the resource the gateway exists to supervise: a ComfyUI daemon is a Python process the operator installs, starts, and sizes outside the gateway's lifecycle, so the gateway cannot bound its VRAM, cannot drain it on a profile switch, and cannot respawn it on death - the precise failure the comparison report itself charges against LocalAI's uncoordinated sidecars (section 4.2 of that report). A managed `sd-server` child keeps image generation inside the same supervision boundary as every other local workload: provisioned from the pinned artifact store, spawned on loopback (sd-server has no auth flag, so the gateway remains the sole credential holder), admitted through a dominion queue, and torn down deterministically at profile switch. If a future requirement demands ComfyUI's workflow programmability (multi-stage graphs, custom nodes), the bridge can return as a second image backend behind the same `Upstream` seam; the endpoint contract in section 3 does not change (high confidence in the first three rejections, which are grounded in the codebase's own structure; medium confidence in the ComfyUI rejection, which trades real workflow flexibility for supervision and could reasonably be revisited if workflow programmability becomes a hard requirement).

## 10. Risks and limitations

The largest technical risk is the timeout policy, assigned in Table 2: until the images path moves from the shared whole-request deadline to a read-timeout policy, long legitimate generations will die as transport errors against both remote and local upstreams. The second risk is the response-body ceiling, also assigned in Table 2; until the dedicated cap lands, large or batched generations fail the same way. The third risk is supervisory: sd-server's unchecked `listen()` means a port-bind failure exits 0, so a supervisor that trusts exit codes will misread a dead child as a clean stop; the readiness-correlation rule in section 5 is the mitigation, and it must survive into the implementation. The fourth risk is provisioning provenance: city96's widely mirrored FLUX GGUFs fail in sd-server with a generic context-creation error, so an operator following popular guides can produce a child that never starts; the leejet-repo pinning guidance in section 5 is the mitigation.

Two lesser risks complete the picture. sd-server's OpenAI compatibility is now source-verified rather than anecdotal - it accepts `prompt`, `n`, `size`, `output_format`, and `output_compression`, silently ignores the other OpenAI fields, and always returns `b64_json` - but the silently-ignored set means a caller asking for `quality` or `background` on a local model gets a compliant-looking response that did not honor the knob; the accept-and-ignore policy in section 4 makes this explicit rather than accidental. And this specification was written against the repository as of 2026-09-03; crate names were recently shortened (`promptforge-gateway` to `gateway`, and siblings), so the implementor should re-verify every cited path and line anchor before editing, because line numbers drift. Finally, this document amends the ComfyUI bridge decision recorded in `report-promptforge-gateway-comparison.md`; if the amendment is accepted, that report's sections 3.6, 5.4, and Table 1 should be updated so the two documents do not disagree. Phase 2's supervisor extraction is specified at the level of intent, not code; the boundary between the shared lifecycle core and per-engine rendering is the one place where the implementor's judgment overrides this document.

---

*2026-09-03 15:15 - kimi-k3*
