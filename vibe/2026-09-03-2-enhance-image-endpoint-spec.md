---
name: Enhance image endpoint spec
overview: "Fold the five-agent web research into design-image-generation-endpoint.md: one factual correction (sd-server auth), four new hazard treatments (timeouts, concurrency, readiness, provisioning), prior-art refinements (validation tables, error-code preservation, payload handling), and an explicit streaming deferral."
todos:
  - id: fix-sd-server-claims
    content: "Revise Section 5: sd-server auth, readiness, exit-code gotcha, single-slot concurrency, lora-model-dir, release pinning"
    status: completed
  - id: fix-wire-contract
    content: "Revise Section 3: response usage/echoed fields, demote style/response_format to rest, defer streaming and edits explicitly"
    status: completed
  - id: timeout-body-policy
    content: Add timeout policy (read_timeout vs total) and payload handling rules (String/RawValue, magic-byte sniff)
    status: completed
  - id: validation-error-mapping
    content: "Add prior-art subsection: per-model validation tables, structured error codes, param policy, no blind retries"
    status: completed
  - id: phase2-operations
    content: Add VRAM budget table, warm-up readiness, GGUF provenance/licensing, progress-gap resolution to Section 5
    status: completed
  - id: supervision-notes
    content: Add supervision implementation notes (tokio::process, Job Objects) for gateway-image
    status: completed
  - id: rederive-summary
    content: Re-derive Executive Summary, Table 1, and Risks from the revised body; run rulebook checklist
    status: completed
  - id: persist-research
    content: Persist research digests to cabinet/_research/2026-09-03-image-endpoint-*.md
    status: completed
isProject: false
---

# Enhance the Image Generation Endpoint Specification

## Goal

Revise [promptforge/design/design-image-generation-endpoint.md](promptforge/design/design-image-generation-endpoint.md) so every claim survives the existing-practice evidence gathered from five research angles (OpenAI Images API, sd-server source, gateway prior art, local serving practice, Rust implementation practice). The two-phase architecture stands; the revisions sharpen correctness, hazards, and operational detail.

## Revision 1: Fix the sd-server supervision claims (Section 5)

Source-verified against stable-diffusion.cpp master (examples/server/runtime.cpp, main.cpp, routes_openai.cpp, async_jobs.cpp):

- Remove "per-attempt credential" for the image child: sd-server has no `--api-key` or any auth flag. Auth stays entirely at the gateway; the child binds loopback only (its default; never pass `--listen-ip 0.0.0.0`).
- Readiness: no `/health` endpoint exists, but the port binds only after weights load (`new_sd_ctx()` runs before `listen()`). Probe = TCP connect or `GET /v1/models`, 60-120 s deadline for FLUX-scale cold loads.
- New hazard: `svr.listen()` return value is unchecked, so a port-bind failure exits 0 after logging "listening on". The supervisor must treat child exit before first successful probe as load failure and disambiguate via captured output, not exit code.
- Concurrency: one child = one generation slot (single `sd_ctx_mutex`; author discourages parallel runs). Gateway queue depth 1 per child; overflow fails fast. Deeper queueing (64-deep) exists only in the native `/sdcpp/v1` async API, which also adds queued-job cancellation and `queue_position` - note it as the path to progress reporting, with its limits (no per-step percent; in-flight jobs not cancellable, 409; completed results expire after 600 s).
- Client disconnect cancels in-flight generation (added April 2026) - aligns with the gateway's drop-cancellation chain; cite it.
- Always pass `--lora-model-dir` explicitly (issue #1468 500s otherwise). Pin rolling release by tag+sha and match assets by pattern (`*-bin-win-cuda12-x64.zip`), never a hardcoded name.

## Revision 2: Correct the wire contract (Section 3)

- Response gains optional `usage` (token breakdown, GPT image models only) and echoed top-level `background`/`output_format`/`quality`/`size`; keep the confirmed "no `model` field" note.
- Demote `style` and `response_format` from named fields to `rest` passthrough: they are dall-e-only and GPT image models reject them with 400. Named request fields become `model`, `prompt`, `n`, `size`, `quality` only.
- Add an explicit deferral: OpenAI image streaming (`stream: true`, `partial_images`, two SSE event types `image_generation.partial_image` / `image_generation.completed`) is out of scope for Phases 1-2 because the gateway's SSE relay is typed on chat chunks; revisit as a Phase 3 with a small dedicated event enum.
- Note `POST /v1/images/edits` (multipart, up to 16 images) as a named future phase; skip `/v1/images/variations` (dall-e-2 legacy).

## Revision 3: Timeout and body-size policy (Sections 5-6)

- The shared 120 s whole-request `REQUEST_TIMEOUT` (`gateway-protocol/src/http_util.rs:20`) is a total deadline that kills legitimate long generations (NVIDIA OpenShell hit exactly this). Spec: connect timeout (10 s) plus reqwest `read_timeout` sized to worst-case generation (120 s default, 300-600 s low-VRAM profile), with dominion queue wait excluded from the generation window.
- Keep the 64 MiB dedicated response cap: field data shows 1024x1024 medium PNG ~1.9 MB base64, 1536x1024 ~2.6 MB, high-quality up to ~6 MB per image, so 64 MiB covers n=10 with headroom.
- Payload handling rule: base64 stays `String`/`RawValue` end to end; never round-trip through `Vec<u8>` serde fields (serde_json array-of-integers expansion). Validate decoded PNG by magic bytes only; no re-encode.

## Revision 4: Validation and error mapping from prior art (new subsection in Section 4)

- Per-model validation tables (allowed sizes, `n` range, prompt length - one-api's three-map pattern) enforced before queue admission so bad requests never burn a slot.
- Preserve upstream structured error codes verbatim (`content_policy_violation`, `moderation_blocked`, `rate_limit_exceeded`); classify by `code`, never message strings (LiteLLM #19328 is the cautionary tale); body-limit rejections are honest 413s.
- Parameter policy: accept-and-ignore cosmetic params (`style`, `user`), hard-400 on capabilities we cannot honor; never silently drop (comfyui-mcp discipline).
- Record upstream request ids and forbid blind retries: upstreams finish and bill after client disconnect (double-billing hazard).

## Revision 5: Operational detail for Phase 2 (Section 5)

- Add the VRAM budget table from measured data: FLUX Q4_K ~8 GB, Q8_0 ~14-15 GB, bf16 ~24 GB; SDXL ~9 GB; SD3.5-Medium ~7-11 GB; Qwen-Image Q4 ~14 GB; Z-Image-Turbo Q4 ~7 GB - all plus 1-2 GB sd-server-vs-CLI headroom, with `--offload-to-cpu` documented as an activation-floor mode needing ~32 GB system RAM.
- Warm-up readiness: after spawn, run one tiny generation (256x256, 1-4 steps) and mark the child ready only when it completes; profile-switch cost is load plus warm-up.
- Provisioning guidance: pin leejet-built FLUX GGUFs (city96 ComfyUI-flavored GGUFs fail with generic `new_sd_ctx_t failed`); companion files (VAE, clip_l, t5xxl / qwen encoder) stay first-class pinned assets; note FLUX.1-dev is non-commercial-gated (needs HF token) while schnell/Qwen-Image/Z-Image are Apache 2.0.
- Progress reporting decision (Table 1) resolves with evidence: sd-server exposes only `queue_position` plus binary `generating` state, so progress is queued-position plus elapsed-time estimate; true per-step bars would require parsing `-v` stderr or patching the server - record as a known gap.

## Revision 6: Supervision implementation notes (Section 5)

- Note the existing `ServerGuard` uses std::process plus supervisor threads; field practice in tokio services prefers `tokio::process` with `select!` on wait-vs-shutdown, and Windows tree-kill wants a Job Object with `KILL_ON_JOB_CLOSE` (`command-group` is the battle-tested crate). Frame as guidance for the new `gateway-image` crate, not a rewrite demand on `gateway-local`.

## Housekeeping

- Update the Executive Summary and Risks (Section 10) to match the revised body: drop the corrected credential claim, add the timeout and exit-code hazards, re-derive per the rulebook.
- Persist the five research digests to `cabinet/_research/` (one file, shared prefix `2026-09-03-image-endpoint-*`) so the report's claims stay auditable.
- Re-run the reports-rulebook section 6 checklist on the revised document.

## Verification

- Every revised claim carries a source (repo file:line or research URL).
- Headings still reconstruct the argument; Table 1 updated in place; no contradiction with `report-promptforge-gateway-comparison.md` beyond the already-flagged ComfyUI amendment.