<!-- source: promptforge codebase analysis and industry prior art research -->
# Report: PromptForge Gateway Version 1.0 - Technical Architecture, Feature Set, and Market Positioning

**Report Type**: Analytical / recommendation report

## 1. Executive Summary

PromptForge Gateway Version 1.0 delivers a unified, high-performance inference boundary for deterministic prompt-as-code pipelines. It integrates text, vision, image generation, speech-to-text, and text-to-speech behind a single OpenAI-compatible API surface. The gateway operates as a single compiled Rust binary with embedded CUDA kernels, dynamic profile hot-swapping, and deterministic VRAM co-residency supervision.

Existing solutions fail to meet the requirements of local and hybrid pipeline execution. Multi-tenant cloud routers (LiteLLM, Portkey) lack local hardware supervision. All-in-one local daemons (LocalAI) suffer from uncoordinated GPU memory allocation and process fragility. Single-purpose model servers (Ollama) omit image generation and audio synthesis. PromptForge Gateway resolves these gaps through its native Rust concurrency substrate, dominion admission queues, and modular workflow bridges.

The gateway ships with complete multimodal coverage. Image generation executes through a gateway-supervised `sd-server` child (stable-diffusion.cpp), which natively speaks the OpenAI images API. Text-to-speech runs locally via Orpheus 3B, a Llama-3B speech-LLM with a SNAC neural codec, served as a managed child or through the gateway's existing llama-server machinery with in-process SNAC decoding. Cloud multimodal calls route directly through standard OpenAI wire passthrough. This architecture preserves a lean binary footprint, enforces strict credential isolation, and guarantees deterministic execution under load.

*Revision note (2026-09-03): this edition supersedes the 2026-08-29 original on two decisions - local image generation moves from an external ComfyUI WebSocket bridge to a managed `sd-server` child, and local text-to-speech moves from Kokoro-82M to Orpheus 3B - per the endpoint specifications `report-image-generation-endpoint.md` and `report-gateway-tts-endpoint.md` in this directory, which carry the full evidence. Crate names have also been shortened since the original (`promptforge-gateway` to `gateway`, and siblings); this edition uses the new names.*

## 2. Strategic Evaluation Criteria

Four criteria govern the evaluation of PromptForge Gateway against existing prior art:

1. **Hardware and VRAM Lifecycle Coordination**: The gateway must coordinate local GPU resources across heterogeneous workloads without triggering CUDA out-of-memory faults.
2. **Deterministic Execution and Concurrency Control**: The system must enforce per-client fair queueing, drop-on-disconnect request cancellation, and bounded waiting queues.
3. **Security Boundary and Credential Isolation**: The gateway must isolate provider credentials from execution runtimes, scrub untrusted headers, and prevent Server-Side Request Forgery.
4. **Protocol Compliance and Operational Overhead**: The architecture must support standard OpenAI wire schemas while maintaining a minimal binary footprint and low latency.

The gateway sits between execution clients and compute resources. It enforces a single admission and isolation boundary:

* **Clients**: Workshop UI, CLI pipelines, and tape replay tools connect via standard OpenAI wire protocols.
* **Gateway Core**: Axum HTTP server with typed SSE relay, constant-time secret authentication, dynamic profile engine, and dominion fair admission queues.
* **Local Hardware Dominion**: Manages `llama-server` child processes with embedded CUDA/Vulkan kernels, multimodal vision projectors, the in-process Whisper STT engine, `sd-server` image generation children, and the Orpheus 3B TTS engine.
* **Remote Provider Pool**: Routes to Anthropic, OpenAI, Brave Search, and cloud image/TTS APIs with credential isolation and passthrough fidelity.

This topology ensures that every inference request passes through a single point of authentication, scheduling, and hardware coordination.

## 3. Technical Anatomy of PromptForge Gateway Version 1.0

PromptForge Gateway Version 1.0 is implemented in Rust using Axum and Tokio. It provides strict security, predictable concurrency, and hardware-managed local inference across all modalities.

### 3.1 Unified OpenAI-Compatible API Surface
The gateway exposes a complete OpenAI-compatible REST surface. Clients interact with a single endpoint for all inference workloads:

* `POST /v1/chat/completions` - Text and vision-language completions with typed SSE streaming.
* `POST /v1/embeddings` - OpenAI-shaped embedding generation for retrieval and similarity.
* `POST /v1/rerank` - Query-document relevance scoring for classifier models.
* `POST /v1/images/generations` - Image synthesis through cloud providers or local `sd-server` children.
* `POST /v1/audio/speech` - Text-to-speech synthesis via cloud APIs or local Orpheus 3B.
* `GET /v1/audio/voices` - Union of the active profile's configured speech voices, in the de-facto OpenAI-compatible shape.
* `POST /v1/audio/transcriptions` - Speech-to-text through the embedded Whisper engine.
* `GET /v1/models` - Bearer-authed model catalog exposing each model's `kind` (`chat`, `embedding`, `classifier`, `image`, `speech`) plus capability metadata (`images`, `parallel_tool_calls`, `effort_levels`, `voices`).
* `POST /v1/tools/web_search` - Authenticated Brave Search proxy with domain filtering and tracking parameter stripping.
* `GET /v1/cache`, `POST /v1/cache`, `DELETE /v1/cache/{sha256}` - Blob cache management for GGUF weights and multimodal projectors.

All routes share the same bearer authentication, model resolution, dominion admission, and error envelope discipline. The gateway validates requests against the OpenAI schema and passes unmodeled parameters through verbatim to upstream backends.

### 3.2 Admission Control, Dominions, and Fair Scheduling
The routing subsystem in `gateway-routing` isolates compute pools using the `DominionQueue` struct. A dominion represents a bounded compute resource: either a remote provider pool or a physical GPU.

```rust
pub struct DominionQueue {
    inner: QueueInner,
}

pub struct ClientId(String);

pub enum AdmitError {
    QueueFull,
    Rejected,
    Unavailable,
}
```

The dominion queue admits callers through a shared permit model. When an endpoint or local model executes, it acquires an `Arc<LimitedQueue>` permit. Dropping the permit on completion or request cancellation releases the slot back to the dominion.

When `fair_scheduling = true` is set, the gateway inspects the `X-PromptForge-Client` header, sanitizing it into a bounded `ClientId` (maximum 64 bytes, restricted ASCII set). Waiting requests are scheduled in per-client round-robin order across up to 32 active buckets, preventing a high-throughput client from monopolizing the inference pool. Full queues either park callers up to `max_queue` (returning HTTP 503 `queue_full` upon saturation) or reject immediately under `policy = "reject"` (returning HTTP 429 `queue_rejected` for fail-fast behavior).

### 3.3 Dynamic Profile Engine and VRAM Budgeting
The gateway loads infrastructure configurations from `gateway.toml` and merges active profiles from sibling `profiles/*.toml` files via `gateway-config`. Profiles resolve include chains depth-first up to 16 levels.

The gateway validates VRAM co-residency before starting processes. When a local dominion sets `vram_gb`, every bound `[[local_model]]` must declare its estimated memory footprint (`vram_gb`). If the aggregate memory exceeds the budget, the gateway rejects the configuration at boot or profile switch before allocating memory.

The admin subsystem exposes `/admin/switch-profile` (`POST`), which executes an atomic profile migration:
1. Validates the candidate profile and verifies that the `[server]` block matches the boot configuration.
2. Acquires a global profile mutex and terminates existing child `llama-server` processes, reclaiming GPU VRAM.
3. Provisions new local model artifacts, downloads missing GGUF weights, and binds child processes to ephemeral loopback ports.
4. Atomically replaces the active routing table and broadcasts stage updates (`loading-profile`, `stopping-models`, `starting-models`, `ready`) over the `/admin/progress` Server-Sent Events stream.

### 3.4 Embedded CUDA Compilation and Local Supervision
Under the `llama-cuda` feature flag on Windows x86-64, `build-llama-cuda` compiles a pinned llama.cpp checkout into a host-native CUDA `llama-server` with CMake. The build harness detects host CUDA capabilities, accounts for the PE dependency closure, copies the CUDA runtime DLLs so the end user needs only the NVIDIA driver, and packs the result into a checksummed release bundle.

At runtime, `gateway-local` provisions pinned `llama-server` release archives - upstream ggml-org builds for most platforms, and a PromptForge-built Blackwell CUDA bundle produced by the `llama-cuda-blackwell` workflow from `build-llama-cuda` - into the operator cache and manages child process lifecycles. Local models support multimodal vision companions via `[local_model.multimodal_projector]`, provisioning projector weights and passing `--mmproj` arguments automatically. If a child crashes mid-flight, the gateway respawns it once on its assigned port and retries the request before returning an error.

### 3.5 Tool Dialect Emulation and Protocol Passthrough
The gateway translates tool calls for models lacking native tool arrays via `tool_dialect = "gemma3_tool_code"`. For non-streaming requests, the gateway converts the standard OpenAI `tools` array into a system instruction guide, strips wire tool fields, and scans incoming model completions for `tool_code` content fences. It parses `name(key=value)` lines into OpenAI `tool_calls` structures. Malformed fences trigger a warn-and-continue recovery mode, setting a `gateway_warning` extension field on the response rather than failing the request.

### 3.6 Image Generation: Managed sd-server Children
The gateway implements `POST /v1/images/generations` in two phases, specified in full in `design/report-image-generation-endpoint.md`. Phase 1 is a pure passthrough: a fourth `ModelKind` (`image`), wire types, an `Upstream::send_images` method that declines by default, and a route mirroring the rerank handler, giving remote generation (OpenAI, Together, any OpenAI-compatible provider) with no local-inference changes. Phase 2 adds local open-weight generation (FLUX.1, SDXL, SD 3.5, Qwen-Image, Z-Image) through a managed `sd-server` child from stable-diffusion.cpp, whose server mode natively speaks the OpenAI images API. The child is provisioned from the pinned artifact store, bound to loopback (sd-server has no auth flag, so the gateway remains the sole credential holder), admitted through a dominion queue, and torn down deterministically at profile switch.

This design amends the original ComfyUI WebSocket bridge. The amendment's grounds are this report's own first criterion: a ComfyUI daemon is a Python process the operator installs and sizes outside the gateway's lifecycle, so the gateway cannot bound its VRAM, drain it on a profile switch, or respawn it on death - the precise failure section 4.2 charges against LocalAI's uncoordinated sidecars. If workflow programmability ever becomes a hard requirement, the bridge can return as a second image backend behind the same `Upstream` seam.

Cloud image generation routes directly through OpenAI, Together, and other OpenAI-shaped providers. The gateway applies the same dominion admission and credential isolation to image workloads as to text completions.

### 3.7 Text-to-Speech: Orpheus 3B Speech-LLM
The gateway serves `POST /v1/audio/speech` as a routed model kind (`kind = "speech"`), specified in full in `design/report-gateway-tts-endpoint.md`. Phase 1 ships remote passthrough (Together AI's Orpheus endpoint, OpenAI's speech models, Mistral's Voxtral API) plus a `GET /v1/audio/voices` discovery route in the de-facto OpenAI-compatible shape. Phase 2 lands local synthesis with Orpheus 3B, a speech-LLM on a Llama-3B backbone emitting SNAC neural-codec tokens, chosen for human-grade conversational prosody after sub-100M phoneme models were evaluated and rejected on acoustic quality. The local engine is selected by a spike between two working native paths: a managed CrispASR child (MIT, prebuilt Windows CUDA binaries, OpenAI-compatible server) and a hybrid that serves the Orpheus GGUF through the existing llama-server machinery with in-process SNAC decoding (candle or ONNX; the decoder is ~25 MB). On the 24 GB target cards, Orpheus at Q8 (~8 GB runtime) co-resides with the whisper STT pair and a 14B-class authoring LLM under the existing dominion VRAM budgeting.

This design amends the original Kokoro-82M choice: Kokoro's acoustic quality - flat cadence, no semantic phrasing, no emotional range - was judged unacceptable for conversational presence, and its 82M-parameter class was rejected outright. Cloud text-to-speech routes through OpenAI (`tts-1`, `gpt-4o-mini-tts`), Together AI, and Mistral passthrough. The gateway streams chunked audio with drop-all-the-way-down cancellation, the same disconnect semantics as the chat relay.

### 3.8 Speech-to-Text: Gateway-Owned Dual-Worker Whisper Pipeline
Speech-to-text lives in the gateway itself, as the `gateway-stt` crate trio: `gateway-stt` owns the HTTP surface (`POST /v1/audio/transcriptions` on the gateway listener, the `/stt` streaming WebSocket on the workshop listener) and the runtime; `gateway-transcribe` owns the engine on dedicated worker threads; `gateway-whisper-ffi` runtime-loads a pinned, digest-verified whisper.cpp shared library provisioned through the artifact store. STT models are first-class catalog entries (`[[stt_model]]` with `interim`/`final` roles), governed by profile membership and dominion VRAM budgets exactly like local chat models. The pipeline:
* **Interim Worker**: Processes sliding audio windows using a lightweight model (`whisper-base-en` or Whisper `large-v3-turbo`) to deliver live streaming feedback.
* **Final-Pass Worker**: Transcribes silence-delimited speech segments with a larger model (`whisper-small-en` or Whisper `large-v3`), conditioning each segment on previous transcript tails.
* **Silence Gating & Glossary Biasing**: Discards audio below an RMS amplitude threshold of 0.001 (-60 dBFS) to prevent Whisper hallucinations and injects domain-specific glossaries into the initial decoding window.

### 3.9 Security and Credential Isolation
The gateway operates as a zero-leakage security boundary:
* **Secret Redaction**: Credentials use the `Secret` type, which redacts plaintext values in `Debug` and `Display` formatters, exposing raw keys only via `expose()`.
* **Constant-Time Token Verification**: Inbound bearer tokens on `/v1/*` and `/admin/*` are hashed with SHA-256 and compared using `subtle::ConstantTimeEq`.
* **Search and Fetch Sanitization**: The built-in Brave Search proxy (`/v1/tools/web_search`) strips tracking parameters (`utm_*`, `fbclid`, `gclid`), enforces host diversity caps (default 2 per host), and applies bare-hostname domain filters. Web retrieval through `promptforge-webfetch` applies four-layer SSRF defenses against internal network probing.

## 4. Prior Art Comparative Analysis

The inference gateway ecosystem divides into three distinct architectural categories. Table 1 summarizes the technical differences across these tools against PromptForge Gateway Version 1.0.

### 4.1 Cloud API Routers and Multi-Tenant Relays
Tools such as LiteLLM, Portkey, One-API, and BricksLLM focus on enterprise API billing, key management, and multi-provider cloud routing. LiteLLM provides a Python FastAPI proxy translating over 100 upstream provider schemas into OpenAI formats. One-API uses a Go and SQL architecture to distribute API keys and meter token budgets across organizations. Portkey implements edge routing with PII sanitization and semantic caching.

These systems excel at cloud governance but lack local compute awareness. They operate under the assumption that backends reside on remote serverless infrastructure with elastic capacity. None of these proxies supervise local GPU processes, monitor VRAM limits, or compile host-native inference kernels. Running them locally introduces high baseline memory usage, Python garbage collection pauses, and multi-container deployment overhead.

### 4.2 All-in-One Local Runtimes
Ollama and LocalAI target self-hosted and local execution environments. Ollama packages a Go daemon around `llama.cpp`, providing a streamlined CLI experience for text and vision models. However, Ollama explicitly omits image generation, audio synthesis, and cloud API fallback routing.

LocalAI attempts complete modality parity by implementing a Go HTTP server that coordinates over 60 isolated backend processes via gRPC (including `llama.cpp`, Python `diffusers`, and `piper`). This architecture suffers from severe operational fragility. Managing dozens of disparate gRPC sidecars leads to orphaned processes and uncoordinated GPU memory allocation. When a client triggers image diffusion while a large language model occupies VRAM, LocalAI cannot arbitrate memory safely, frequently causing fatal CUDA out-of-memory crashes.

### 4.3 Specialized Multimodal Bridges
Standalone utilities such as `Comfyui2Openai`, `sd-webui-openai`, and `Kokoro-FastAPI` translate standard OpenAI image and audio endpoints into custom local tool calls. ComfyUI bridges connect over WebSockets to inject prompt and dimension parameters into JSON workflow graphs. Audio proxies wrap Kokoro-82M or Piper behind `/v1/audio/speech`.

These tools solve narrow translation tasks but operate as fragmented single-purpose scripts. They lack unified authentication, per-client admission queues, and centralized configuration management.

Table 1 details how PromptForge Gateway Version 1.0 compares across core operational criteria against existing prior art.

| System | Runtime Core | Modalities Supported | Local Engine Management | VRAM & Hardware Supervision | Concurrency & Queueing | Profile Hot-Swapping |
|---|---|---|---|---|---|---|
| **PromptForge Gateway 1.0** | **Rust (Axum + Tokio)** | **Text, Vision, Embeddings, Rerank, Search, Image Gen, TTS, STT** | **Pinned `llama-server` children + in-process Whisper + Orpheus 3B + managed `sd-server`** | **Deterministic VRAM bounds (`vram_gb`)** | **Dominion fair queues (`X-PromptForge-Client`)** | **Zero-downtime SSE profile migration** |
| **LiteLLM** | Python (FastAPI) | Text, Vision, Audio, DALL-E | None (external daemons only) | None | Basic RPM / token semaphores | Static config / DB reload |
| **Portkey** | TypeScript (Node.js) | Text, Vision, Embeddings | None (cloud only) | None | Cloud-managed rate limits | Control plane dashboard |
| **One-API / New-API** | Go + SQL | Text, Vision, Midjourney, Audio | None (external relays only) | None | Channel round-robin retries | Web UI database update |
| **LocalAI** | Go + gRPC | Text, Vision, Image Gen, TTS, STT | Polyglot gRPC (60+ processes) | Uncoordinated (frequent OOM) | Unbounded LRU eviction | Static YAML gallery files |
| **Ollama** | Go + C++ | Text, Vision (`mmproj`) | Bundled `llama.cpp` | Static GPU layer offloading | Bounded parallel slots | CLI / Modelfile reload |
| **ComfyUI Bridges** | Python / Rust | Image Generation only | External ComfyUI daemon | Delegated to ComfyUI | Single-worker FIFO | Hardcoded workflow JSON |

Table 1: Comparative capability matrix across major inference gateways, local model runners, and multimodal routers.

## 5. Value-Add Analysis and Strategic Differentiation

PromptForge Gateway Version 1.0 establishes a defensible market position through architectural specialization rather than feature parity. The gateway serves deterministic prompt-as-code pipelines with hardware-aware supervision that generic proxies cannot replicate.

### 5.1 Protocol Compliance Versus Operational Redundancy
Adopting OpenAI wire schemas (`POST /v1/images/generations`, `POST /v1/audio/speech`) is an interoperability requirement rather than code duplication. Serving these endpoints enables the PromptForge Workshop UI, CLI pipelines, and external IDE extensions to use standardized client libraries without custom transport adapters.

Commoditization applies exclusively to the outward wire interface. The internal routing logic, hardware coordination, and memory budgeting remain unique to PromptForge.

### 5.2 The Core Moat: Deterministic Pipeline Supervision
PromptForge Gateway is engineered to serve deterministic prompt-as-code pipelines defined in Markdown. When complex multi-step pipelines execute parallel fanouts or adversarial review passes, unmanaged gateways fail under upstream rate limits or local GPU exhaustion.

PromptForge Gateway protects pipeline execution through three core mechanisms:
1. **Dominion Concurrency Queues**: Compute pools isolate heavy multimodal tasks from real-time text completions. Bounded queues prevent noisy neighbors from exhausting backend capacity.
2. **Deterministic VRAM Co-Residency**: By validating `vram_gb` footprints across models, the gateway guarantees that local image diffusion and language models never oversubscribe physical GPU memory.
3. **Atomic Profile Transitions**: The gateway swaps entire model fleets over SSE streams (`/admin/switch-profile`), enabling pipelines to transition seamlessly from reasoning stages to image synthesis stages without dropping connections.

### 5.3 Low-Latency Rust Infrastructure
Python and Node.js proxies incur substantial garbage collection overhead, memory bloat, and runtime vulnerabilities. PromptForge Gateway executes as a single compiled Rust binary with zero-allocation SSE chunk relaying, strict type validation, and constant-time secret comparison. This provides the microsecond-level routing efficiency required for intensive local and hybrid pipelines.

### 5.4 Modular Multimodal Architecture
The gateway achieves complete modality coverage without monolithic bloat:
* **Image Generation**: Managed `sd-server` children (stable-diffusion.cpp) speak the OpenAI images API natively, keeping multi-gigabyte Python dependencies out of the gateway's supply chain while staying inside the supervision boundary - provisioned, VRAM-budgeted, queue-admitted, and torn down at profile switch.
* **Text-to-Speech**: Orpheus 3B, a 3B-parameter speech-LLM, delivers human-grade conversational prosody with guided emotion control at ~8 GB runtime, co-resident with the resident LLMs on 24 GB cards under dominion VRAM budgeting.
* **Speech-to-Text**: The gateway-owned dual-worker Whisper pipeline provides sub-200ms interim streaming with silence-gated final passes and domain vocabulary biasing.

This modular design keeps the core binary under 50MB while supporting every major inference modality.

## 6. Market Positioning and Competitive Advantages

PromptForge Gateway Version 1.0 occupies a unique niche at the intersection of local hardware supervision and cloud API governance. No existing solution combines these capabilities in a single deployable unit.

### 6.1 Against Cloud-Only Routers
LiteLLM and Portkey serve enterprise platform teams managing cloud spend. They cannot coordinate local GPU memory, compile host-native kernels, or hot-swap model profiles. PromptForge Gateway serves developers and researchers running hybrid local-cloud pipelines who require deterministic execution and hardware safety.

### 6.2 Against Local-Only Daemons
Ollama and LocalAI serve hobbyists and home-lab users. Ollama lacks multimodal coverage and cloud routing. LocalAI lacks memory coordination and process stability. PromptForge Gateway serves professional pipelines where a CUDA out-of-memory crash or a dropped connection invalidates hours of deterministic computation.

### 6.3 Against Single-Purpose Bridges
ComfyUI bridges and Kokoro wrappers solve narrow translation tasks. They lack unified authentication, admission control, and profile management. PromptForge Gateway integrates these capabilities into a single supervised boundary with consistent security and scheduling policies.

## 7. Conclusion

PromptForge Gateway Version 1.0 is not a redundant clone of existing proxy software. It is a specialized inference boundary engineered for deterministic prompt-as-code pipelines. The gateway combines OpenAI protocol compliance with Rust-native performance, dominion-based hardware supervision, and modular multimodal bridges. This architecture delivers capabilities that cloud routers cannot provide and local daemons cannot stabilize.

The gateway ships as a finished product: a single binary that manages text, vision, image generation, speech synthesis, and speech recognition across local GPUs and cloud providers with deterministic reliability.

---

*2026-08-29 18:35 - Gemini 3.7 Flash. Revised 2026-09-03 15:30 - kimi-k3: image generation amended to managed sd-server children, text-to-speech amended to Orpheus 3B, STT updated to the gateway-owned crate trio, crate names shortened, per the endpoint specifications in this directory.*
