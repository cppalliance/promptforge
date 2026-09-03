---
name: Enhance TTS endpoint report
overview: "Revise promptforge/design/report-gateway-tts-endpoint.md with the findings from four web-research streams plus the Voxtral TTS analysis: correct outdated claims, rebuild the phase-2 engine options around newly discovered native runtimes, extend the wire spec, and add the missing risks."
todos:
  - id: pass-1-corrections
    content: "Pass 1: correct outdated claims (native runtimes, Together dialect, VRAM budget, cloning scope, llama.cpp forecast)"
    status: completed
  - id: pass-2-engine-options
    content: "Pass 2: rebuild Table 1 with five engine options incl. hybrid and Voxtral"
    status: completed
  - id: pass-3-wire-spec
    content: "Pass 3: extend wire spec (stream_format, instructions, voice object, WAV header, transcode policy, input cap, angle-bracket rule)"
    status: completed
  - id: pass-4-voices-endpoint
    content: "Pass 4: add GET /v1/audio/voices decision and per-checkpoint voices note"
    status: completed
  - id: pass-5-risks
    content: "Pass 5: rewrite Risks (SNAC frame-drop, concurrency bottleneck, TTFA floor, sampling, Windows contention, licenses, drift evidence, conditioning prior art)"
    status: completed
  - id: pass-6-client-infra
    content: "Pass 6: client playback path and reqwest/axum infra details"
    status: completed
  - id: pass-7-plan-tests-audit
    content: "Pass 7: update implementation plan + test plan, rulebook audit, footer"
    status: completed
isProject: false
---

# Enhance the Gateway TTS Endpoint Report

Target file: `promptforge/design/report-gateway-tts-endpoint.md`. Evidence base: three research files in the workspace research store (`2026-09-03-conduct-research-openai-tts-api-surface.md`, `2026-09-03-conduct-research-tts-gateway-conventions.md`, `2026-09-03-web-research-tts-streaming-practice.md`, plus the Orpheus native-serving findings) and the Voxtral verification from this session. Every edit preserves the reports-rulebook discipline: answer-first, citations, likelihood separate from confidence.

## Pass 1 - Correct outdated factual claims

- Executive summary and Section 5: replace "no maintained native Rust Orpheus runtime exists" - candle merged SNAC (PR #2869) and a complete Orpheus example (PR #2886, Apache/MIT); rlx-orpheus 0.2.11 exists but is GPL-3.0-only (copyleft trap, flag explicitly); CrispASR (MIT, C++/ggml, v0.8.31, 2026-08-31) ships prebuilt self-contained Windows x86-64 CUDA zips with OpenAI-compatible `/v1/audio/speech`, streaming PCM, and a C ABI with Rust bindings.
- Table 1, Option A: "orpheus-cpp" is a Python package (llama-cpp-python + PyTorch SNAC), not a native binary. Replace candidate with CrispASR; new risk is single-author packaging defects (v0.8.29 SIGILL incident), mitigated by the ArtifactStore's existing pin-and-digest discipline.
- Section 4: qualify the Together AI claim - OpenAI-compatible but not identical: `response_format` defaults to `wav` (not mp3), adds `raw`/`mulaw`/`sample_rate`/`bit_rate`/`language`/`stream`; streaming is SSE of base64 raw PCM (`stream=true` + `response_format=raw`), not chunked binary, so verbatim passthrough forfeits streaming from Together. Scope phase 1 to non-streaming passthrough plus OpenAI-style chunked binary; SSE decode is a fast follow.
- Background, decision 2: Orpheus Q8 runtime footprint is ~8 GB (weights + SNAC decoder + KV at 8192 ctx), not ~4.5 GB. The 24 GB stack still fits but with essentially no headroom; soften "fits with headroom" and change the example config to `vram_gb = 8`.
- Background, decision 1: zero-shot cloning works only on the Orpheus *pretrained* base, not the `-ft` 8-voice model being deployed. Correct the implication; cloning is out of scope for both phases.
- Section 5 forecast: downgrade llama.cpp Orpheus/SNAC support from "roughly even chance" to "unlikely on a useful timeline" - issue #12476 open 18 months, draft PR #12487 stalled since 2025-05, maintainer wants a generalized codec design first.

## Pass 2 - Rebuild the phase-2 engine options (Table 1)

Five options, new ordering:
- A. Managed child: CrispASR prebuilt binary (MIT, Windows CUDA, OpenAI-shaped, streaming PCM) supervised by the existing ServerGuard machinery. Strongest low-effort path.
- B. Hybrid (new, arguably lowest risk): serve the Orpheus GGUF via the existing llama-server child as a plain text model (works in stock llama.cpp today) and decode SNAC in-process via candle (`candle-transformers::models::snac`) or ONNX (`laion/SNAC-24khz-decoder-onnx`). Decoder is ~25 MB F32; reuses all lifecycle/VRAM/dominion machinery untouched.
- C. In-process FFI: CrispASR's shared-library build + C ABI with Rust bindings means the "purpose-built shared library" may already exist; rlx-orpheus proves the pure-Rust path but is GPL-3.0-only.
- D. Extend llama.cpp: downgrade, stalled upstream.
- E. Voxtral-4B-TTS-2603 (new alternative engine): CC BY-NC 4.0 (non-commercial; commercial needs a Mistral agreement), 16-21 GB VRAM BF16 (breaks single-24GB-card co-residency with the authoring LLM; the Talktron demo needed two 3090s), serving is vLLM-Omni (Python) only - no llama.cpp/GGUF path. Strengths: 20 voice presets, working 3-second zero-shot cloning, 9 languages, Mistral API passthrough at $0.016/1k chars. Document as remote-passthrough provider and engine alternative; keep Orpheus as the phase-2 target.
- Spike acceptance test: ASR-roundtrip or reference-WAV comparison, not "produces sound" - the 7-token SNAC deinterleave with per-position offsets (0/4096/.../24576) fails silently as silent audio.

## Pass 3 - Extend the wire protocol spec (Section 2)

- Add `stream_format` (`sse`|`audio`) and `instructions` (gpt-4o-mini-tts style control; a local Orpheus backend may map it to prompt conditioning) as optional fields; document that `voice` is validated as a string against the catalog even though OpenAI now also accepts `{"id": "..."}` custom-voice objects.
- Add the content-type mapping table (mp3 -> `audio/mpeg`, wav -> `audio/wav`, pcm -> `audio/pcm`; opus/aac spellings verified per provider at implementation time).
- WAV streaming decision: emit the 44-byte header with 0xFFFFFFFF size placeholders (hound's `into_header_for_infinite_file()`; this is what OpenAI itself streams), then raw PCM. No RF64. Strict parsers should request `pcm`.
- Transcode policy when the local engine emits 24 kHz PCM: pcm = passthrough; wav = header prepend; mp3 = stream-encode (rusty_mp3 or glint-audio, spike first); opus = ropus into Ogg; aac/flac = 400 in phase 1. No resampling anywhere - Orpheus, OpenAI pcm, and Together raw are all 24 kHz mono.
- Input cap: keep 4096 chars (matches OpenAI and CrispASR's default), but note the binding local constraint is tokens - Orpheus quality degrades past ~4k generated tokens despite 8192 training length - and state that long-input chunking belongs to the follow-on conditioning helper, not the route.
- Hard rule, stated in bold: never sanitize angle-bracket content out of `input` - Orpheus emotion tags (`<laugh>`, `<sigh>`, etc.) arrive inline.

## Pass 4 - Catalog and voices endpoint (Sections 1 and 6)

- Keep the `voices` capability on `GET /v1/models` and add a trivial `GET /v1/audio/voices` route rendering the union of configured speech models' voices - the de-facto compat standard that off-the-shelf clients (Open WebUI, Talemate) probe for. Serve `{"id","name"}` objects; the Kokoro-FastAPI v0.4.0/Open WebUI breakage (issue #462) shows entry shape is itself a compatibility surface.
- Note the `finetune-prod` multilingual Orpheus variants exist if `voices` should be per-checkpoint.

## Pass 5 - Rewrite the Risks section

- Replace the vague "autoregressive hallucination" item with the named SNAC frame-drop defect: reference decoder silently drops frames with out-of-range codes, skipping words; acknowledged upstream, unfixed; community fix is slot-constrained logits. Engine choice is a quality gate, not just feasibility.
- Add: SNAC decode, not the LLM, is the concurrency bottleneck when colocated on one GPU (orpheus-streaming measurements); RTF 1.0 floor is ~91 codec tokens/sec; streaming stutter is a throughput symptom.
- Add: TTFA realism - SNAC's non-causal decode sets a floor; Canopy's "~200ms" marketing referred to input streaming; expect ~150-250 ms on 24 GB cards.
- Add: sampling stability - greedy is unstable, pin temp ~0.6 + top-k as speech-model defaults rather than inheriting chat defaults.
- Compute contention: remove the CUDA-stream-priority mitigation (per-process only; NVIDIA MPS is Linux-only, useless on the Windows target). Replace with a priority lane for speech in the existing dominion queue plus client-side prebuffer (400-600 ms, not the 1-2 frames / ~85-170 ms the draft suggested) plus TTFA instrumentation against a 100-300 ms stage budget.
- Add license footnote: orpheus-3b-0.1-ft is tagged Apache-2.0 but derives from Llama-3.2-3B (Llama 3.2 Community License); SNAC is MIT; Voxtral is CC BY-NC 4.0. Compliance note for redistributing GGUFs through the artifact store.
- Provider dialect drift: upgrade to partially-confirmed with the dated example - the gpt-4o-mini-tts 2025-12-15 snapshot ignores `instructions` and truncates final sentences while the working snapshot is deprecated.
- Text-conditioning follow-on: cite prior art (wyoming_tts_proxy strip rules, TextForSpeech identifier/path verbalization, mendelio-voice-text's display/spoken/model three-representation pattern).

## Pass 6 - Client playback and infra details

- Section 6 follow-on: primary playback path is `response_format=pcm` + fetch ReadableStream + AudioWorklet port-queue player with 400-600 ms prebuffer; MSE `audio/mpeg` sequence-mode fallback; WebView2 needs `--autoplay-policy=no-user-gesture-required`.
- Section 2/3 infra: add `read_timeout` (30-60 s idle, resets per read) + `tcp_keepalive` to the streaming reqwest client so a dead upstream cannot hold a dominion permit forever; exclude the route from any future Compression/Timeout layers; never set Content-Length on the stream.

## Pass 7 - Plan, tests, and rulebook audit

- Implementation plan: add the `GET /v1/audio/voices` step; phase 2 spike now evaluates CrispASR managed-child vs the candle/ONNX hybrid against the ASR-roundtrip acceptance test.
- Test plan: add SNAC layer-parity fixtures (rlx-orpheus's `ref_codes.json`/`ref_stage_*.npy` pattern) under engine-gated tests.
- Final pass: re-derive the executive summary and headings from the revised body, re-run the reports-rulebook section 6 checklist, update the footer timestamp.

## Verification

Read the final report top to bottom; confirm every corrected claim carries a source (URL or file:line), every new risk is rated, and the recommendation chain (routed kind -> phase 1 remote -> phase 2 spike between CrispASR and the hybrid) reads consistently end to end.