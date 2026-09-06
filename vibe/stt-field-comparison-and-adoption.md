# Realtime STT Architecture: Field Comparison and Adoption Priorities

Report type: evaluation / review. It judges PromptForge against five open-source codebases sharing realtime speech-to-text architecture and prescribes idioms to adopt, in payoff order.

## Executive summary

PromptForge has a strong native core wrapped in an unsafe service boundary. Its FFI ownership, dedicated inference workers, and generation-aware transcript protocol beat much of the field, but the absence of a backend seam, bounded admission, and per-session final-pass ownership makes the current design brittle under extension and concurrency. The highest-payoff change is to preserve the two-model stable-plus-unstable algorithm while moving model execution behind a provider-neutral contract.

### Key findings

1. **Steal Vox and Dalston's backend boundary.** Separate the realtime pipeline from physical model runtimes, with whisper.cpp as the first adapter. Confidence: high.
2. **Steal bounded pressure control from all five references.** Every audio, inference, and session queue needs a declared limit and a typed overload outcome. Confidence: high.
3. **Steal owned sessions from Vox, GigaSTT, and Dalston.** Per-session take handles prevent concurrent clients from resetting each other's final-pass state. Confidence: high.
4. **Steal GigaSTT's atomic engine publication.** Requests must observe one complete model generation, not independently updated runtime fields. Confidence: medium.
5. **Steal Dalston's native protocol plus edge translators.** Gateway events should describe transcription facts, while Workshop derives UI status. Confidence: high.
6. **Steal GigaSTT's bounded shutdown order.** Stop ingress, close queues, drain under a deadline, emit one terminal outcome, then release native state. Confidence: high.
7. **Steal provider-neutral CI tests from Universal Realtime STT.** A deterministic fake backend should exercise all critical behavior without model fixtures. Confidence: high.

## Method

PromptForge was profiled first through nine architecture lenses, with speech-to-text weighted above unrelated code. Fifteen open-source candidates were surveyed and fourteen were verified against source; five complementary references were selected and examined at pinned commits. Each cited idiom received a provenance tag, including pre-AI rewinds where explicit markers appeared. Findings were ranked by deficit severity, convergence, and adoption cost, then 27 citations were checked against the pinned clones; nine path corrections were applied and no finding was dropped.

## Reference projects and provenance

| Reference | Popularity | Why chosen | License | Provenance of cited idioms |
|---|---:|---|---|---|
| [GigaSTT](https://github.com/ekhodzitsky/gigastt) | 49 stars | Closest complete Rust streaming server | MIT | 5 strong human signal |
| [Vox](https://github.com/mrtozner/vox) | 43 stars | Backend and per-session streaming abstractions | MIT OR Apache-2.0 | 6 strong human signal |
| [Keyless](https://github.com/hate/keyless) | 25 stars | Bounded queues and single-owner inference | MIT | 5 strong human signal |
| [Universal Realtime STT](https://github.com/Chronica-Anima/universal-realtime-stt) | 2 stars | Small provider lifecycle contract | MIT | 3 strong human signal, 3 unknown |
| [Dalston](https://github.com/ssarunic/dalston) | 2 stars | Native protocol, lag policy, and session lifecycle | Apache-2.0 | 1 strong human signal, 5 explicit AI marker |

## Baseline: where the subject stands

PromptForge is a Rust-first STT stack built from Axum, Tokio, WebSockets, dedicated whisper.cpp worker threads, and a runtime-loaded C ABI. Its strongest mechanisms are dedicated blocking workers in `gateway-transcribe/src/worker.rs` and `final_pass.rs`, RAII wrappers in `gateway-whisper-ffi/src/library.rs` and `context.rs`, generation-aware frames in `gateway-stt/src/stt.rs`, and layered errors across the FFI, transcription, and Gateway crates.

The main deficits are concrete. `gateway-transcribe/src/engine.rs`, `worker.rs`, and `final_pass.rs` hard-code whisper.cpp rather than a backend contract. `gateway-stt/src/stt.rs` and both worker modules use unbounded buffers or queues. `gateway-stt/src/runtime.rs` publishes engine and model-name state separately, waits without a shutdown bound, and silently resets active takes during profile switches. The final-pass worker owns one global current take, so simultaneous realtime clients can interleave resets and segment notifications. Rust and TypeScript duplicate the wire schema, the browser permits a new take while finalization is pending, and model-dependent integration tests are skipped in normal CI.

## Detailed findings, ranked by payoff

### Finding 1: Separate the realtime pipeline from model backends

Vox splits batch STT from per-session streaming through backend-neutral traits ([`src/traits.rs:38-114`](https://github.com/mrtozner/vox/blob/fd6f2abd1b55340e2c5f50551fee939172557825/src/traits.rs#L38-L114)). Universal Realtime STT keeps transport mapping inside provider adapters ([`stt_provider.py:76-108`](https://github.com/Chronica-Anima/universal-realtime-stt/blob/c3ce5b164154b10d6b46b58f24bb0c5714ed4b21/universal_realtime_stt_tts/stt_provider.py#L76-L108)). Dalston defines canonical request and transcript types before engine adapters ([`base.py:170-213`](https://github.com/ssarunic/dalston/blob/04c99b307d7b7563c6e7be711b1f48447cde9814/dalston/realtime_sdk/base.py#L170-L213), [`base_transcribe.py:24-135`](https://github.com/ssarunic/dalston/blob/04c99b307d7b7563c6e7be711b1f48447cde9814/dalston/realtime_sdk/base_transcribe.py#L24-L135)). Keyless corroborates the boundary with its vendor-neutral transcriber contract ([`transcriber.rs:36-68`](https://github.com/hate/keyless/blob/4cefbea3755b6ad10757cc713b08fe639f40f9b6/keyless-whisper/src/transcriber.rs#L36-L68)).

Replace direct whisper.cpp assumptions in `gateway-transcribe/src/engine.rs`, `worker.rs`, and `final_pass.rs` with batch and realtime transcription interfaces. Keep the sliding window, silence segmentation, stable prefix, unstable suffix, and accurate final pass in the pipeline above those interfaces. Implement whisper.cpp first and add no public backend selector until a second adapter exists. Confidence: high - four references converge on the same boundary, and it directly addresses the highest-leverage deficit.

### Finding 2: Bound every queue and make overload a protocol outcome

GigaSTT ties inference capacity to an owned pool permit ([`inference/pool.rs`](https://github.com/ekhodzitsky/gigastt/blob/da75d72bcbcf8b1ec908648ef0be29664983f436/crates/gigastt-core/src/inference/pool.rs)). Vox exposes queue and parallelism limits ([`streaming_pipeline.rs:40-217`](https://github.com/mrtozner/vox/blob/fd6f2abd1b55340e2c5f50551fee939172557825/src/streaming_pipeline.rs#L40-L217)). Keyless deliberately chooses loss behavior at bounded audio ingress ([`cpal.rs:134-186`](https://github.com/hate/keyless/blob/4cefbea3755b6ad10757cc713b08fe639f40f9b6/keyless-audio/src/input/cpal.rs#L134-L186)). Universal Realtime STT uses timed producer backpressure ([`stream_wav.py:17-31`](https://github.com/Chronica-Anima/universal-realtime-stt/blob/c3ce5b164154b10d6b46b58f24bb0c5714ed4b21/helpers/stream_wav.py#L17-L31)). Dalston measures lag in audio time and terminates after warning plus grace ([`session.py:1406-1564`](https://github.com/ssarunic/dalston/blob/04c99b307d7b7563c6e7be711b1f48447cde9814/dalston/realtime_sdk/session.py#L1406-L1564)).

Replace the growing audio vectors and unbounded worker channels in `gateway-stt/src/stt.rs`, `gateway-transcribe/src/worker.rs`, and `final_pass.rs` with declared limits. Apply backpressure before admission, retain final results ahead of disposable hypotheses, and return a retryable structured overload event when latency or memory crosses the budget. Confidence: high - every reference independently treats unbounded pressure as a correctness problem.

### Finding 3: Give every stream an owned session and completion guard

Vox gives each native stream a drop-safe owner ([`sherpa_streaming.rs:93-292`](https://github.com/mrtozner/vox/blob/fd6f2abd1b55340e2c5f50551fee939172557825/src/stt/sherpa_streaming.rs#L93-L292)). GigaSTT makes scarce capacity an RAII-owned resource ([`inference/pool.rs`](https://github.com/ekhodzitsky/gigastt/blob/da75d72bcbcf8b1ec908648ef0be29664983f436/crates/gigastt-core/src/inference/pool.rs)). Dalston centralizes session allocation, keepalive, release, and finalization ([`realtime_proxy.py:79-254`](https://github.com/ssarunic/dalston/blob/04c99b307d7b7563c6e7be711b1f48447cde9814/dalston/gateway/services/realtime_proxy.py#L79-L254)).

The final worker in `gateway-transcribe/src/final_pass.rs` currently owns one mutable transcript and one completion channel for the current take. Replace that global take with an owned handle keyed by connection and item. Its drop path must cancel queued work, remove accumulated transcript state, close completion delivery, and return capacity. This is required before multiple realtime clients are safe. Confidence: high - ownership is the convergent protection against leaks and cross-session corruption.

### Finding 4: Publish model generations atomically

GigaSTT builds one engine aggregate and swaps it through `ArcSwap`, so requests see either the old complete generation or the new one ([`state.rs`](https://github.com/ekhodzitsky/gigastt/blob/da75d72bcbcf8b1ec908648ef0be29664983f436/crates/gigastt/src/server/http/state.rs)). Dalston uses validated capability metadata during worker selection ([`engine.yaml:1-46`](https://github.com/ssarunic/dalston/blob/04c99b307d7b7563c6e7be711b1f48447cde9814/engines/stt-transcribe/faster-whisper/engine.yaml#L1-L46), [`_realtime_common.py:149-300`](https://github.com/ssarunic/dalston/blob/04c99b307d7b7563c6e7be711b1f48447cde9814/dalston/gateway/api/v1/_realtime_common.py#L149-L300)).

`gateway-stt/src/runtime.rs` should publish the engine, physical model identities, logical pipeline identity, capabilities, and generation as one immutable snapshot. A profile switch should let an existing session finish against its captured generation or send a typed terminal event; it must not silently clear a take. Confidence: medium - GigaSTT supplies the exact atomic mechanism, while Dalston corroborates capability-driven selection but exhibits metadata drift.

### Finding 5: Keep one typed native protocol and translate only at edges

Dalston keeps provider-neutral events inside the realtime core and translates compatibility dialects at the public route ([`protocol.py:60-393`](https://github.com/ssarunic/dalston/blob/04c99b307d7b7563c6e7be711b1f48447cde9814/dalston/realtime_sdk/protocol.py#L60-L393), [`realtime.py:1100-1229`](https://github.com/ssarunic/dalston/blob/04c99b307d7b7563c6e7be711b1f48447cde9814/dalston/gateway/api/v1/realtime.py#L1100-L1229)). GigaSTT versions its capability handshake and typed failures ([`protocol/mod.rs`](https://github.com/ekhodzitsky/gigastt/blob/da75d72bcbcf8b1ec908648ef0be29664983f436/crates/gigastt-core/src/protocol/mod.rs)). Universal Realtime STT records transport policy and normalizes provider events through one pump ([ADR 0001](https://github.com/Chronica-Anima/universal-realtime-stt/blob/c3ce5b164154b10d6b46b58f24bb0c5714ed4b21/doc/adr/0001%20Use%20Official%20SDKs%20for%20ElevenLabs%20and%20Speechmatics%20STT.md), [`_event_queue.py:12-119`](https://github.com/Chronica-Anima/universal-realtime-stt/blob/c3ce5b164154b10d6b46b58f24bb0c5714ed4b21/universal_realtime_stt_tts/_event_queue.py#L12-L119)).

Define one native transcription event model for session creation, stable text, revisable hypotheses, completion, failure, overload, profile change, and termination. Translate it to the OpenAI Realtime subset at Gateway's public edge. Workshop should relay those public events and map them into UI text locally, so `gateway-stt/src/stt.rs` contains no `Push`, `Activity`, `workshop_status`, or Workshop-specific header. Keep Rust and TypeScript aligned through shared fixtures instead of handwritten parallel schemas. Confidence: high - three references separate native meaning from boundary dialects.

### Finding 6: Make shutdown ordered, bounded, and observable

GigaSTT cancels producers before draining tasks under a deadline ([`listen.rs`](https://github.com/ekhodzitsky/gigastt/blob/da75d72bcbcf8b1ec908648ef0be29664983f436/crates/gigastt/src/server/listen.rs)). Vox pairs cancellation signals with owned completion handles ([`live_talk.rs:24-515`](https://github.com/mrtozner/vox/blob/fd6f2abd1b55340e2c5f50551fee939172557825/src/server/live_talk.rs#L24-L515)). Dalston's shared proxy core owns allocation through final release ([`realtime_proxy.py:79-254`](https://github.com/ssarunic/dalston/blob/04c99b307d7b7563c6e7be711b1f48447cde9814/dalston/gateway/services/realtime_proxy.py#L79-L254)).

Replace the unbounded strong-count wait in `gateway-stt/src/runtime.rs` with a shutdown sequence: reject new audio, cancel session producers, close worker queues, await final jobs under a deadline, send one terminal outcome, and release model state. Native inference may remain non-preemptible, but it must not hold the process or a session forever. Confidence: high - the references agree on ownership and order even where native cancellation remains impossible.

### Finding 7: Test the provider contract without native models

Universal Realtime STT drives provider orchestration through deterministic doubles ([`tests/test_unit.py:24-263`](https://github.com/Chronica-Anima/universal-realtime-stt/blob/c3ce5b164154b10d6b46b58f24bb0c5714ed4b21/tests/test_unit.py#L24-L263)). GigaSTT enforces runtime-factory isolation in CI ([`runtime/factory.rs`](https://github.com/ekhodzitsky/gigastt/blob/da75d72bcbcf8b1ec908648ef0be29664983f436/crates/gigastt-core/src/runtime/factory.rs), [CI lines 212-222](https://github.com/ekhodzitsky/gigastt/blob/da75d72bcbcf8b1ec908648ef0be29664983f436/.github/workflows/ci.yml#L212-L222)).

Add a fake backend that runs in every CI job and deterministically covers batch selection, hypothesis replacement, stable deltas, completion, overload, cancellation, profile changes, and concurrent session isolation. Keep real Whisper model tests as a second integration tier, not the only proof of critical behavior. Confidence: high - this directly closes PromptForge's skipped-test gap without weakening native coverage.

## Provenance

Five cited Dalston mechanisms carry explicit AI markers. Its canonical transcript contract was AI-originated with no pre-AI form; the edge translators at HEAD tightened an older proxy form from `ddd1b11a181683a7dcb1c748226267b9ec0540ed`; the lag budget remained present against `77316618ab802563f823bc336e71522c6dc3def1`; bounded ingress was added after that same earlier session form; and capability metadata tightened from `eade4b733a0bdaa164e5f744703f2755e24a5ebb`. GigaSTT, Vox, and Keyless cited mechanisms carry strong human signals; Universal Realtime STT is mixed between strong human signal and unknown.

## Where the subject already matches or beats the references

- PromptForge isolates blocking native inference on dedicated threads as cleanly as Keyless and more clearly than Vox's cancellation path.
- PromptForge's `gateway-whisper-ffi` RAII wrappers provide stronger native pointer and dynamic-library lifetime ownership than any shortlisted reference exposed.
- PromptForge's generation-aware `stream`, `interim`, and `final` frames already reject stale restart output, a stronger explicit invariant than the small provider adapter projects.
- PromptForge preserves source errors through FFI, transcription, and Gateway layers, while Vox collapses many backend failures to strings.
- PromptForge's worker and runtime ownership is structural; it should preserve that strength when adding bounded session handles.

## Messes we should explicitly not copy

- GigaSTT leaves timed-out native inference detached while retaining its pool slot, silently ignores malformed controls, and keeps protocol drift checks advisory.
- Vox concentrates four protocols in a 1,429-line WebSocket module, cannot interrupt blocking inference, and erases backend error structure.
- Keyless leaks desktop bridge threads across pipeline restarts, treats log strings as an internal protocol, and can let finalization overtake queued audio.
- Universal Realtime STT can lose terminal sentinels and final transcripts when queues fill, suppresses sender failures, and cannot terminate one provider's blocking shutdown thread.
- Dalston concentrates session responsibilities in a 1,621-line module, unloads engines without awaiting active sessions, and advertises streaming capability that its runtime does not implement.

## Recommended execution order

1. Define native batch, realtime, session, hypothesis, terminal, and overload contracts, then add the deterministic fake backend. This establishes Findings 1, 5, and 7 before moving behavior.
2. Refactor whisper.cpp behind the backend contract without changing the existing two-model algorithm. Verify one-shot and realtime equivalence.
3. Replace global final-take state with owned per-session handles and add concurrent-client tests from Finding 3.
4. Add bounded audio, worker, and admission queues with explicit overload behavior from Finding 2.
5. Publish runtime generations atomically and define profile-switch outcomes from Finding 4.
6. Add ordered bounded shutdown from Finding 6.
7. Translate the native events to the OpenAI Realtime endpoint, make Workshop an opaque relay, and derive UI status locally.

## Refactor notes

Findings 1 and 5 begin as structural changes, but backend substitution, protocol translation, queue limits, and switch outcomes change behavior and require characterization tests first. Preserve `gateway-whisper-ffi` ownership and the two-model stable-plus-unstable algorithm. Do not place Workshop types or status text in Gateway crates. Commit each execution-order item after its focused tests pass; two consecutive failures on one item stop the run for a re-plan. The full test suite is the invariant and moves last.

## Sources

- GigaSTT, https://github.com/ekhodzitsky/gigastt, `da75d72bcbcf8b1ec908648ef0be29664983f436`, MIT, analyzed 2026-09-05.
- Vox, https://github.com/mrtozner/vox, `fd6f2abd1b55340e2c5f50551fee939172557825`, MIT OR Apache-2.0, analyzed 2026-09-05.
- Keyless, https://github.com/hate/keyless, `4cefbea3755b6ad10757cc713b08fe639f40f9b6`, MIT, analyzed 2026-09-05.
- Universal Realtime STT, https://github.com/Chronica-Anima/universal-realtime-stt, `c3ce5b164154b10d6b46b58f24bb0c5714ed4b21`, MIT, analyzed 2026-09-05.
- Dalston, https://github.com/ssarunic/dalston, `04c99b307d7b7563c6e7be711b1f48447cde9814`, Apache-2.0, analyzed 2026-09-05. Rewinds: `ddd1b11a181683a7dcb1c748226267b9ec0540ed`, `77316618ab802563f823bc336e71522c6dc3def1`, `eade4b733a0bdaa164e5f744703f2755e24a5ebb`.
- Field survey and PromptForge profile produced 2026-09-05.

*2026-09-05 07:55 - GPT-5.6 Sol*
