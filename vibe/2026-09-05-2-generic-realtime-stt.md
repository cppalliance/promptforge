---
name: generic-realtime-stt
overview: Replace Workshop-specific speech transcription with a generic OpenAI Realtime-compatible subsystem built around a small Gateway facade, a backend-neutral engine, a safe Whisper backend, and isolated bounded session ownership.
todos:
  - id: characterize
    content: Pin current batch and two-model realtime behavior with deterministic and native fixtures
    status: pending
  - id: backend-boundary
    content: Split the backend-neutral engine from the safe Whisper adapter and bound model workers
    status: pending
  - id: session-lifecycle
    content: Isolate per-item finalization and implement atomic cancellation-safe generation replacement
    status: pending
  - id: realtime-contract
    content: Implement the OpenAI Realtime transcription subset and hypothesis extension
    status: pending
  - id: workshop-adapter
    content: Convert Workshop to a payload-opaque relay with local capture and status ownership
    status: pending
  - id: verify-document
    content: Enforce architecture and debt budgets, complete acceptance, and document final boundaries
    status: pending
  - id: gateway-log-bookends
    content: Mark Gateway serving-log launch and terminal outcomes without changing logging infrastructure
    status: pending
  - id: ci-architecture-toolchain
    content: Run pinned architecture tools under the repository Cargo version inside stable CI
    status: completed
  - id: ci-workshop-sidecars
    content: Stage target-named Gateway sidecars before Windows and Linux Workshop CI builds
    status: completed
  - id: ci-native-rustup
    content: Use the self-hosted Windows runner's preinstalled Rust without reinstalling rustup
    status: completed
  - id: windows-cache-sid
    content: Restrict Windows artifact caches to the current process SID for users and service accounts
    status: completed
  - id: ci-session-retirement
    content: Make session retirement verification event-driven instead of scheduler-yield-counted
    status: completed
  - id: ci-gateway-platform-warnings
    content: Restore warnings-denied Gateway builds on non-Windows hosts
    status: completed
  - id: chat-model-selection
    content: Prevent silent built-in chat turns when no model is selected
    status: completed
  - id: workshop-startup-convergence
    content: Converge model catalog, Realtime readiness, and progress state after simultaneous startup
    status: completed
  - id: realtime-live-hypotheses
    content: Bind precommit hypothesis item IDs to the active browser take
    status: pending
isProject: false
---

# Generic Realtime STT

## Product Requirements

- Problem and users:
  - PromptForge Gateway speech transcription is coupled to Workshop through custom routes, status frames, guards, and crate dependencies.
  - The current two-model realtime implementation has global final-pass state, unbounded queues, backend-specific engine code, and profile-switch waits that cannot prove isolation or bounded completion.
  - Workshop users need responsive dictation, external clients need a stable OpenAI-shaped transcription protocol, and Gateway maintainers need compiler-visible product and backend boundaries.
- Goals:
  - Preserve `POST /v1/audio/transcriptions` for batch transcription by physical model name.
  - Replace `/stt` and `/stt/capability` with `WS /v1/realtime?intent=transcription` using the supported OpenAI Realtime transcription event subset plus one documented hypothesis extension.
  - Preserve the existing fast interim model, LocalAgreement-2, and accurate segment-final model behavior while making every session and committed item independent.
  - Let clients share one loaded interim worker and at most one loaded final worker without copying models per client.
  - Make Gateway depend on one small speech facade, keep the engine backend-neutral, and preserve the unsafe-only Whisper FFI leaf.
  - Make Workshop a payload-opaque authenticated relay whose UI owns microphone capture, hypothesis presentation, and status wording.
  - Reduce and enforce technical debt through dependency allowlists, module-cycle checks, bounded queues, public-surface budgets, and line-count ratchets.
  - Close the operator-requested Gateway observability gap by making every serving file log start with a versioned launch record and end with a clean or fatal terminal record unless the process is killed.
- Non-goals:
  - Dynamic backend plugins before a second backend exists.
  - AlignAtt, attention-specific APIs, native streaming encoders, speculative decoding, batching, denoising, generic VAD tuning, or a new WER framework.
  - A fifth STT crate or STT wire types in `shared-protocol`.
  - Gateway CLI behavior, diagnostics, logging queues, sink lifecycle, log rotation, or the already-owned Workshop baseline-ratchet repair. The two serving-run bookend records in Step 41 are the sole logging exception.
  - Browser-direct OpenAI authentication or WebRTC. Workshop remains the browser authentication proxy.
- Success criteria:
  - The forbidden `gateway-stt -> workshop-server` dependency falls from one to zero and no Gateway STT crate contains Workshop-specific status or guard symbols.
  - The final subsystem has exactly four STT crates: service, engine, safe Whisper backend, and unsafe-only FFI.
  - All STT queues and session mailboxes are bounded, named, and tested; global mutable final-take state is zero.
  - Multiple clients and overlapping committed items cannot exchange transcript history, completion channels, prompts, or stale results.
  - The only live transcription endpoint is `/v1/realtime?intent=transcription`; batch transcription remains compatible.
  - Default Gateway builds no longer transitively build Workshop UI assets.
  - `gateway-stt` exports no free functions and at most six facade types; `gateway-stt-engine` exports at most eight root items; `gateway-stt-backend-whisper` exports exactly its backend and checked configuration.
  - Every final STT source module is at most 500 physical lines, with ratchets preventing regrowth.
  - Model-independent critical behavior runs in normal CI, native characterization passes with the packaged Whisper runtime, and synthetic Gateway, Workshop browser, packaged Windows microphone, cancellation, and second-take acceptance pass.
- Constraints:
  - `gateway-whisper-ffi` remains the only STT crate permitted to contain unsafe code, load C symbols, own native pointers, or encode ABI layout.
  - The public wire format is signed little-endian mono PCM16 at 24 kHz; the engine receives 16 kHz mono `f32` after one stateful conversion.
  - Native decode and model loading are non-preemptible. Profile replacement must never pretend to kill native work or load a second generation beside live old-generation model state.
  - New profile admission, old-generation draining, persistence, activation, rollback, and fatal shutdown must have explicit ownership and bounded control-plane behavior.
  - The existing two-second closing silence, 500 ms interim cadence, 15-second interim window, 500 ms minimum decode, model pair, and decode policy remain unchanged during this structural migration.
- Open questions:
  - None.

## Functional Specification

- Actors and workflows:
  - A batch client uploads audio to `POST /v1/audio/transcriptions` and selects one active physical interim or final model.
  - A Realtime client connects with exactly one transcription intent, receives `session.created`, optionally updates the effective transcription session, appends Base64 PCM16, observes negotiated live hypotheses, commits an input turn, and receives immediate item creation followed by asynchronous delta, completed, or failed events.
  - Workshop accepts a same-origin browser socket, attaches the Gateway bearer upstream, forwards text and binary payloads without parsing JSON, and preserves close code and reason.
  - The Workshop UI captures 24 kHz PCM16, renders each hypothesis as a replacement snapshot, replaces it with the authoritative completed transcript, and derives status locally.
  - A profile switch closes generation admission, notifies old sessions, drains request and worker ownership, stages the replacement, coordinates profile persistence, then activates, rolls back, or performs controlled shutdown.
- Inputs and outputs:
  - Supported client events are `session.update`, `input_audio_buffer.append`, `input_audio_buffer.commit`, and `input_audio_buffer.clear`.
  - Supported server events are `session.created`, `session.updated`, `input_audio_buffer.committed`, `input_audio_buffer.cleared`, `conversation.item.created`, transcription delta, completed, failed, hypothesis, and error events.
  - Session responses include `id`, `object: "realtime.transcription_session"`, `type: "transcription"`, the complete effective audio configuration, and `include`.
  - Completion includes authoritative transcript and duration usage. Client event IDs are optional opaque strings echoed only in correlated errors; server event, session, and item IDs are independently generated opaque strings.
  - The hypothesis extension contains revision, full transcript, finalized, agreed, tentative, and audio-span fields. Its transcript equals the exact concatenation of its three text components.
- States and validation:
  - A connection starts ready with the advertised default logical model, 24 kHz PCM, null turn detection, and no optional includes. A valid update atomically replaces the session default and returns the full effective configuration.
  - Each input buffer snapshots immutable format, model, prompt, and include configuration on its first append. Prompt changes affect the next buffer and never mutate an existing input or committed item.
  - `turn_detection: null` is the sole supported turn-detection value. Non-null VAD, noise reduction, keywords, languages, delay, logprobs, unsupported models, and unknown include values are rejected without partially applying an update.
  - The custom hypothesis include value is a PromptForge extension. When it is negotiated, Workshop ignores standard deltas for rendering and uses hypothesis replacement until completion.
  - An input buffer owns its provisional item ID, audio and resampler state, interim task, LocalAgreement state, hypothesis, accurate final take, and any pending precommit failure.
  - Commit reserves committed-item, terminal-mailbox, and bounded task-join capacity before changing the input. On success it seals interim work, promotes the same provisional item ID, records `previous_item_id` from durable commit order, emits committed and item-created immediately, and finalizes asynchronously.
  - Up to four committed items per connection may finalize concurrently. Completion order may differ from commit order, and durable lineage survives removal of completed items.
  - Clear cancels and retires only uncommitted work, resets partial PCM and resampler state, emits cleared, and leaves committed items untouched.
  - Appends are limited to 15 MiB decoded audio; commits require at least 100 ms; unfinalized audio is capped at 30 seconds; excessive queue lag, item count, buffer size, or queue occupancy produces explicit overload behavior.
- Errors and recovery:
  - Malformed JSON, unsupported fields, invalid PCM, short commits, unknown models, and safe overloads emit correlated errors while keeping the connection usable when state remains valid.
  - If accurate precommit work fails, the input records a pending failure and rejects further appends. Commit first establishes the item and then emits exactly one item-scoped failure; clear discards the pending failure without inventing an item.
  - A committed item whose authoritative segment cannot be admitted fails atomically rather than completing with a transcript hole.
  - Socket sends have deadlines. Hypotheses may coalesce newest-wins, but accepted delta and terminal results are not internally dropped while the peer remains writable. End-to-end receipt is not claimed without peer acknowledgment.
  - Engine replacement fails every committed in-flight item, emits a general error for uncommitted audio, and closes the session with code 1012 and reason `engine_replaced`.
  - If old-generation work does not drain by the switch deadline, the old generation reopens with a fresh session epoch and replacement fails without closing worker ingress or loading new model memory.
  - Ordinary staged-load or persistence failure rolls back by shutting the new generation down and reconstructing the old generation. Non-preemptible startup timeout or indeterminate persistence outcome leaves rollback unsafe and triggers controlled Gateway shutdown.
- Security and privacy behavior:
  - Gateway applies its existing bearer, verified-cookie, or trusted-loopback authentication policy. Workshop always attaches the bearer upstream.
  - Gateway accepts only absent Origin for native clients or HTTP loopback origins under its named loopback policy. Workshop separately requires the normalized browser Origin authority to match its validated request authority.
  - Missing, duplicate, unknown, or conflicting Realtime routing parameters are rejected before upgrade. Workshop constructs the fixed upstream transcription target rather than forwarding arbitrary query text.
  - The relay preserves message type, close code, and close reason, defines ping and pong ownership, and rejects unsupported subprotocol negotiation.
  - PCM, Base64 audio, transcript text, prompts, vocabulary, credentials, cookies, request headers, and full local model paths never enter tracing fields.
- Acceptance criteria:
  - Canonical event fixtures round-trip in Rust and are consumed unchanged by Workshop UI tests.
  - Sequence tests prove first-event readiness, optional client IDs, immediate commit acknowledgment, provisional-ID promotion, configuration snapshot isolation, durable lineage, reversed completion order, clear semantics, and saturated-commit retry.
  - Two clients sharing one engine remain isolated under interleaved interim, final, clear, commit, failure, and profile-switch activity.
  - Packaged Windows Workshop records, revises hypotheses, completes, starts a second take, cancels a take, and reports permission or device failure as recoverable.

## Technical Design

- Architecture:
  - `gateway` owns route mounting, authentication policy, profile-switch orchestration, operational status, and model catalog integration.
  - `gateway-stt` owns artifact preparation, the `SpeechService` facade, active-generation lifecycle, batch and Realtime routes, session and item orchestration, take guidance, finalized history, segmentation, LocalAgreement state, transcript aggregation, completion and failure, wire translation, and error mapping.
  - `gateway-stt-engine` owns backend-neutral decoder contracts, one bounded serialized worker per loaded physical model, stateless bounded decode jobs, worker cancellation, and engine policy. It owns no session, item, take, guidance history, transcript aggregation, completion channel, or item failure state.
  - `gateway-stt-backend-whisper` owns safe Whisper model construction, prompt fitting, decode parameters, native-load progress, and backend error translation.
  - `gateway-whisper-ffi` remains the unchanged unsafe-only runtime-loaded ABI leaf.
  - `workshop-server` owns only the authenticated payload-opaque relay and generic speech-status mapping. Its UI owns capture and presentation.
  - Core dependency direction is `gateway -> gateway-stt -> gateway-stt-engine`, `gateway-stt -> gateway-stt-backend-whisper`, and `gateway-stt-backend-whisper -> gateway-stt-engine + gateway-whisper-ffi`. No edge points from a Gateway STT crate to Workshop.
- Modules and interfaces:
  - `SpeechService` is the cloneable Gateway handle. Its public supporting types are `PreparedSpeech`, opaque `SpeechReplacement`, `SpeechError`, `SpeechStatus`, and `SpeechModelInfo`; route handlers, wire structs, state, engines, and constants stay private.
  - The speech lifecycle surface prepares verified artifacts, begins a serialized staged replacement, commits or aborts that replacement, invalidates staged state during fatal shutdown, reports status and models, returns routes, and shuts down. Gateway never selects workers or constructs wire events.
  - The engine exports only `SttEngine`, `EnginePolicy`, one-method `ModelFactory`, one-method `Decoder`, `DecodeRequest`, `DecodeMode`, and `TranscribeError`.
  - `DecodeRequest` carries one stateless decode job. `gateway-stt` owns immutable user guidance and finalized transcript history, derives the request prompt for each job, and never leaves that state in a decoder or worker. A decoder need not be sendable; construction and every decode occur on its owning worker thread.
  - The safe Whisper backend exports only its backend and checked configuration. It exposes no FFI pointer, C symbol, session, route, profile, or Workshop type.
  - Cross-crate access uses explicit crate-root re-exports. Internal modules remain private and public fields remain private.
  - Shared loopback code owns separately named Gateway loopback-Origin and Workshop same-origin-authority predicates. Their security semantics are not conflated.
- File and public API changes:
  - Rename `crates/gateway-transcribe` to `crates/gateway-stt-engine` without a compatibility crate. Move its native fixtures and ignore rule with it.
  - Add `crates/gateway-stt-backend-whisper` and move safe model construction, prompts, native parameters, and progress reporting out of the engine.
  - Preserve `crates/gateway-whisper-ffi` API and ABI tests. Do not move scheduling, prompt policy, model roles, or HTTP concepts into it.
  - Replace the `gateway-stt` monolith with responsibility-named runtime, batch, Realtime session, audio, wire, and take modules. Add shared canonical Realtime fixtures under its tests.
  - Replace `SttRuntime`, `SttState`, independent model-name locks, and slot publication with the speech facade and one complete engine snapshot.
  - Move tuning from runtime use of `WorkshopSttConfig` and `[workshop.stt]` to `SttPipelineConfig` and canonical `[stt]`. Legacy input is accepted only when canonical input is absent, both forms together are invalid, and serialization writes only canonical form.
  - Extend model metadata with transcription kind. Active physical names remain selectable for batch use and one logical `realtime-transcribe` model is advertised only while the pair is active.
  - Extend generic Gateway operational status with configured, ready, GPU, and generation speech fields. A build without STT reports no speech object.
  - Add Workshop `routes/realtime.rs` and a Realtime Gateway connector beside `routes/stt.rs`, the old connector, status parsing, and old UI. Convert the worklet to little-endian PCM16 and migrate the UI only after the additive relay passes; remove the old path only after installed-package microphone acceptance.
  - Replace Workshop's old STT capability proxy with a Workshop-local dictation capability derived from generic Gateway speech status.
  - Remove `/stt`, `/stt/capability`, Workshop status frames, custom status headers, legacy route exports, and the Workshop dependency only after the new Gateway route and Workshop consumer pass automated and physical-microphone acceptance.
  - Treat `AGENTS.md` files as concise local constraints, not duplicate architecture documents. Delete obsolete ownership, dependency, route, configuration, and compatibility rules in the commit that makes them false; add only the minimum crate-specific rule needed to protect a new boundary.
  - Keep root `AGENTS.md` and already-correct nested rules unchanged unless implementation exposes a concrete contradiction. The final rules audit prefers removing stale text over expanding rule files.
- Data, persistence, failure, security, and privacy constraints:
  - One interim worker and optional final worker are shared by all clients. `gateway-stt-engine` owns `INTERIM_JOB_CAPACITY = 8` and `FINAL_JOB_CAPACITY = 8`; both are bounded synchronous queues with nonblocking overload responses, and opening sessions never creates OS threads.
  - Each admitted worker job owns a generation work guard until cancellation is observed before decode or native decode returns. Request cancellation cannot make quiescence report false idleness.
  - `gateway-stt::take::Take` is the only take abstraction. Each input or committed item owns one `Take` containing immutable guidance, finalized history, segment aggregation, completion, and failure. Final-model workers remain stateless between jobs.
  - `gateway-stt` owns `MAX_ACTIVE_REALTIME_SESSIONS = 8` with no waiting admission queue and immediate rejection of the ninth session, plus `MAX_COMMITTED_ITEMS_PER_SESSION = 4`.
  - Interim task epochs prevent post-commit or post-clear results from allocating event IDs or mutating later items. `gateway-stt` owns `SESSION_CANCEL_JOIN_CAPACITY = 8`; cancelled task handles are retained and joined through that bounded session-owned capacity.
  - `gateway-stt` owns `SESSION_RESULT_CAPACITY = 16` plus one separately reserved terminal slot per committed item, one replaceable newest-wins hypothesis slot per item, and `FINAL_SEGMENT_CAPACITY = 4` per item. Authoritative segments and terminal outcomes do not use lossy admission.
  - Audio decoding preserves odd-byte state, decodes little-endian samples explicitly, resamples continuously from 24 kHz to 16 kHz, flushes on commit, and fully resets on clear.
  - Active snapshot publication includes generation, physical names, backend, engine, and admission state in one lock-bounded transition. Batch and Realtime admission borrow one complete generation.
  - Every generation has an admission gate, explicit request and job ownership counts, and a replaceable session epoch. Quiescence installs a fresh epoch for possible rollback, cancels the old epoch, and waits for all old ownership to drain.
  - Replacement is serialized. Artifact preparation starts no worker. After old work drains, old workers shut down without detachment, the new generation loads under one startup deadline, and it remains unpublished until profile persistence succeeds.
  - Profile persistence prepares and syncs a temporary file before destructive replacement, atomically replaces the authoritative file after staging, and syncs the parent where supported. Profile reads remain serialized with publication.
  - Determinate failure aborts the staged generation and reconstructs the old specification. Indeterminate persistence or non-preemptible startup timeout consumes staged state, invalidates the replacement token, and initiates controlled process shutdown.
  - Profile replacement never detaches a live native worker. Final process exit may abandon a non-preemptible call only as an explicitly reported last resort, without claiming model memory or callbacks were released.
  - Every named bound has capacity and capacity-plus-one tests owned by its defining module. The fixed bounds also retain 15 MiB per append, 30 seconds unfinalized audio, and two seconds acceptable audio lag. Bounds remain code policy until measurements justify configuration.

## Testing Plan

- Unit:
  - Pin current batch and two-model behavior before moving code, including append-only accurate segments, replaceable provisional text, final authority, silence, segment ordering, and policy constants.
  - Use role-specific scripted fake decoders to test thread confinement, exact request mode, guidance and history propagation, queue admission, cancellation, worker loss, factory error, panic, startup timeout classification, and partial-construction cleanup.
  - Test LocalAgreement token comparison, exact whitespace ownership, hypothesis revision and duplicate suppression, final authority, stale epoch rejection, and revision overflow handling.
  - Test little-endian PCM known bytes, Base64 boundaries, odd-byte appends, continuous resampling, commit flush, clear reset, duration calculation, append limit, short commit, and maximum buffered audio.
  - Test immutable configuration snapshots, null-only turn detection, unsupported fields, optional client IDs, exact query validation, item lineage, reserve-before-detach, saturated retry, pending precommit failure, and one terminal outcome.
  - Test generation admission races, fresh epoch after rollback, queued and running job guards, replacement cancellation at every await, replace against replace, replace against shutdown, determinate rollback, fatal token invalidation, and idempotent shutdown.
- Integration and end-to-end:
  - Round-trip every canonical client and server fixture and drive sequence fixtures for ready creation, update, append, clear, commit acknowledgment, item creation, overlapping items, reversed completion, failure, and error correlation.
  - Run native Whisper characterization before and after the engine and backend split using the same packaged runtime, model, audio, and expected transcript.
  - Exercise batch physical-model selection, authentication, body limits, operational status, model listing, loopback and same-origin checks, and featureless Gateway compilation.
  - Start Gateway with a scripted engine and verify the mounted Realtime route from connect through hypothesis and completion while legacy `/stt` still works.
  - Drive Gateway and Workshop independently from the same canonical fixture sequences. Gateway tests inject scripted decoders without depending on `workshop-server`; Workshop relay tests inject an upstream fixture without depending on `gateway` or `gateway-stt`. The installed Windows package is the real dual-server acceptance.
  - Drive the real Workshop dictation UI with fake media and worklet inputs, including second take, clear, overlapping finalization, hypothesis replacement, completion replacement, recoverable errors, status, and cleanup.
  - Build packaged Windows binaries and perform the new-path microphone gate before legacy removal, then repeat final record, second-take, cancel, and permission or device-failure acceptance before completion.
- Regression, security, and performance:
  - Enforce an exact workspace dependency allowlist for Gateway, all four STT crates, shared loopback, and Workshop server. Temporary rename-only engine edges to FFI and progress expire when the safe backend takes ownership; the Workshop edge expires at legacy removal.
  - Enforce acyclic production-library module graphs for all four STT crates with compiler-resolved `cargo-modules` output, and line-count ceilings for every STT source module. Register each new module when created and never grow the legacy monolith before deleting it.
  - Keep architecture checks in the default Gateway CI path so Workshop job exclusions cannot skip them. Prove Gateway-only builds no longer invoke Workshop UI tooling after legacy removal.
  - Pin and wire Miri in a dedicated earlier step, then run pure ownership, queue, audio-state, agreement, and replacement-state targets under it. Keep sockets, dynamic FFI, native callbacks, and model loading on native CI.
  - Test foreign, malformed, wrong-port, and mismatched loopback origins; missing Origin for native clients; trusted-loopback and strict-auth modes; duplicate or conflicting query parameters; and payload privacy.
  - Test exact saturation boundaries for session, committed-item, interim, final, segment, mailbox, and cancellation-join capacities. Park native-equivalent fake work to prove bounded switch and shutdown behavior without sleeps.
  - Capture first-provisional, first-agreed, endpoint, queue, compute, and final latency plus maximum queue depth and overload counts. Changes to constants require measured latency, memory, and transcript-quality evidence.
- Exit criteria:
  - Formatting, linting with warnings denied, workspace tests, documentation tests, dependency audit, module architecture checks, both UI suites, Gateway and Workshop builds, featureless Gateway check, native Whisper tests, and synthetic full-path tests pass.
  - Debt budgets are recorded before and after and every zero or cap target is met rather than deferred.
  - Gateway-only builds contain no Workshop UI build edge, Gateway STT crates contain no Workshop dependency, and custom live STT routes and status messages are absent.
  - Browser dictation and packaged Windows microphone acceptance pass before legacy removal and again at final completion.

## Decision Record

- Decisions:
  - The operator requirement, "I want to make sure that whatever we build is generic because what you've done is you've, now you've tied Gateway to Workshop," settles the product boundary: Gateway exposes generic speech facts and Workshop remains only a consumer.
  - The operator requirement, "I want you to prove that the technical debt's going to go down," settles measurable debt budgets, exact dependency enforcement, queue bounds, public API caps, and module ratchets as completion criteria.
  - The operator requirement, "I want to have a well-defined boundary between all those syntax code and the gateway," settles the service, backend-neutral engine, safe Whisper backend, and unsafe-only FFI split.
  - Use an OpenAI Realtime transcription subset for standard clients and one optional hypothesis snapshot extension for PromptForge's three-level live transcript.
  - Name the extension negotiation value `item.input_audio_transcription.hypothesis` and its server event `conversation.item.input_audio_transcription.hypothesis`; these literals are version-one public wire commitments pinned by canonical fixtures.
  - Pin the public contract to the OpenAI Realtime transcription schema retrieved 2026-09-05 from `https://developers.openai.com/api/docs/guides/realtime-transcription` and the generated OpenAI Node schema at commit `e228aaad`, especially `src/resources/realtime/realtime.ts` and `src/resources/realtime/client-secrets.ts`. This plan implements only the strict subset below; unknown fields are rejected on client events, unsupported upstream options are rejected as specified, and unsupported optional server fields are omitted.
  - Standard client event `session.update`: required `type: "session.update"` and `session`; optional opaque client `event_id`. `session` requires `type: "transcription"` and may contain `audio` and `include`. `audio` may contain only `input`; `input` may contain `format: {"type":"audio/pcm","rate":24000}`, `noise_reduction: null`, `transcription` with `model: "realtime-transcribe"` and string `prompt`, and `turn_detection: null`. `include` may contain only `item.input_audio_transcription.hypothesis`. Omitted supported values retain their previous effective values. Non-null noise reduction or turn detection, `language`, logprobs, keywords, delay, another model, another format, an unknown include, and every other upstream field are rejected atomically.
  - Standard client event `input_audio_buffer.append`: required `type: "input_audio_buffer.append"` and Base64 `audio`; optional opaque client `event_id`; no other fields.
  - Standard client event `input_audio_buffer.commit`: required `type: "input_audio_buffer.commit"`; optional opaque client `event_id`; no other fields.
  - Standard client event `input_audio_buffer.clear`: required `type: "input_audio_buffer.clear"`; optional opaque client `event_id`; no other fields.
  - Standard server events `session.created` and `session.updated`: required `event_id`, the respective `type` literal, and complete effective `session`; no optional event fields. The effective session requires `id`, `object: "realtime.transcription_session"`, `type: "transcription"`, `audio: {"input":...}`, and `include`. Effective input requires the fixed PCM format object, `noise_reduction: null`, `transcription` with logical model and current prompt, and `turn_detection: null`; `include` is an array containing zero or one negotiated hypothesis value. `expires_at`, client secrets, modalities, output audio, language, logprobs, and other upstream session fields are omitted.
  - Standard server event `input_audio_buffer.committed`: required `event_id`, `type: "input_audio_buffer.committed"`, provisional `item_id`, and `previous_item_id` as an opaque item ID or null; no optional fields.
  - Standard server event `input_audio_buffer.cleared`: required `event_id` and `type: "input_audio_buffer.cleared"`; no optional fields.
  - Standard server event `conversation.item.created`: required `event_id`, `type: "conversation.item.created"`, `previous_item_id` as an opaque item ID or null, and `item`. The item requires the same committed `id`, `type: "message"`, `status: "completed"`, `role: "user"`, and one `content` entry `{"type":"input_audio","transcript":null}`; audio bytes and all other conversation item variants or fields are omitted.
  - Standard server event `conversation.item.input_audio_transcription.delta`: required `event_id`, its exact `type`, `item_id`, `content_index: 0`, and string `delta`; logprobs and other optional upstream fields are omitted.
  - Standard server event `conversation.item.input_audio_transcription.completed`: required `event_id`, its exact `type`, `item_id`, `content_index: 0`, authoritative string `transcript`, and `usage` with `type: "duration"` and nonnegative numeric `seconds`; token usage, languages, logprobs, and other optional upstream fields are omitted.
  - Standard server event `conversation.item.input_audio_transcription.failed`: required `event_id`, its exact `type`, `item_id`, `content_index: 0`, and `error`. The nested error requires string `type`, string `code`, and string `message`; optional `param` is a string or null. No transcript or usage is emitted.
  - Standard server event `error`: required server `event_id`, `type: "error"`, and `error`. The nested error requires string `type`, string `code`, and string `message`; optional `param` is a string or null and optional `event_id` is the opaque client event ID or null. Client event IDs appear nowhere else.
  - Custom server event `conversation.item.input_audio_transcription.hypothesis`: required `event_id`, its exact `type`, `item_id`, `content_index: 0`, monotonically increasing unsigned `revision`, full `transcript`, `finalized`, `agreed`, `tentative`, nonnegative `audio_start_ms`, and nonnegative `audio_end_ms`; no optional fields. The three text components concatenate byte-for-byte to `transcript`, the span is half-open, and this event is emitted only when its include value was negotiated.
  - Server event, session, and item IDs are independently generated, nonempty opaque strings. A provisional item ID is allocated before commit and promoted unchanged; durable commit order alone determines `previous_item_id`; server IDs are never derived from or equal by contract to a client event ID.
  - Preserve four STT crates. A second backend may implement the engine contracts later without changing HTTP or WebSocket endpoints.
  - Share one serialized worker per loaded physical model while owning audio, agreement, history, finalization, and failures per input or committed item.
  - Keep Workshop's Rust relay payload-opaque and derive all UI status locally from capture and protocol events.
  - Use explicit generation admission, request and job guards, session epochs, and a two-phase replacement token instead of strong-reference counts or lock guards crossing awaits.
  - Treat non-preemptible native startup timeout and indeterminate profile persistence as fatal controlled-shutdown cases rather than claiming unsafe rollback.
  - Use Miri from pinned `nightly-2026-09-05` for pure STT ownership, queue, audio, agreement, and replacement tests. A dedicated workflow and Cargo feature-filtered targets establish this repository-selected UB interpreter before the final verification step.
  - Architecture enforcement uses authoritative tools instead of interpreting full Rust syntax itself. Cargo metadata supplies workspace edges, the inherited compiler lint `unsafe_code = "forbid"` supplies unsafe isolation, `cargo-modules` 0.25.0 supplies expanded production-library module edges, and `cargo-public-api` 0.52.0 supplies effective public exports. A small Node 22 driver checks tool versions, module cycles, and public-root budgets; the Rust integration test owns only dependency policy, strict ceiling files, exact migration targets, and lint inheritance. Falsifier: either pinned tool disagrees with rustdoc or Cargo on an adversarial fixture, fails on a supported CI platform, or requires a newer compiler than Rust 1.89.
  - Add two Gateway serving-log bookends because the operator identified an observability gap after the closed `gateway-logging-cli` run: the first file record identifies process version and launch, and the last record distinguishes clean or fatal exit from a killed process. This exception changes no CLI path, queue, sink, retention, rotation, redaction, subscriber ownership, or no-subscriber behavior.
  - Run `cargo-modules` 0.25.0 and `cargo-public-api` 0.52.0 under the repository Rust 1.89 toolchain even when the surrounding CI job tests current stable. Cargo 1.98 removed the unstable metadata argument used by the pinned module tool, while Cargo 1.89 is the architecture contract's supported toolchain. The architecture driver owns this isolation so local and CI invocations cannot drift with ambient stable.
  - Compile-only Workshop CI stages a real featureless Gateway binary under Tauri's target-suffixed `externalBin` name before compiling Workshop, then removes it. Release and nightly packaging continue staging the full release Gateway through their existing paths; no placeholder binary, checked-in artifact, or Tauri bundle change is accepted.
  - The self-hosted Windows native runner must use Rust already provisioned under its service account. Add that account's Cargo bin directory to `PATH`, verify its `rustup`, `cargo`, and stable toolchain, and fail with a runner-provisioning error when any is absent. Do not run a rustup installer on the persistent runner or modify its default toolchain.
  - Windows private-cache enforcement identifies the current process by its token SID rather than `USERNAME` and `USERDOMAIN`. Resolve the SID through the standard `whoami /user /fo csv /nh` interface, validate its canonical SID shape, and pass it to `icacls` with the required `*` SID prefix. Fail closed when identity resolution or ACL verification fails; never special-case or weaken privacy for service accounts.
  - Session retirement tests wait on an explicit registry cleanup signal under a real deadline rather than counting scheduler yields. A slow CI scheduler must not make a correct bounded cleanup test fail, and a missing cleanup signal must still time out visibly.
  - Gateway host-specific declarations and lint expectations exist only on the platforms that use them. Non-Windows builds must not compile the Windows manifest constant or carry an unfulfilled unsafe-code expectation.
  - Installed-package STT acceptance uses a local unsigned NSIS build with updater artifacts disabled only through the Tauri command-line configuration override. Functional acceptance does not require distribution signing. Repository release configuration and release CI signing remain unchanged, and the acceptance record must state that signing was not tested.
  - Agent input cannot enter the built-in chat while no model is selected. The UI keeps text editable, blocks submission, and names the required model selection; if selection becomes invalid after submission, `models.chat` emits the existing model-turn failure observation before Lua `pcall` returns to the next input. A local model-binding error must never appear as a silent tool result with no Gateway request.
  - Workshop startup must converge after it launches beside a still-loading Gateway. While Gateway remains reachable, an empty model catalog or absent selection triggers bounded refresh retries; a failed initial Realtime socket reconnects under bounded backoff; completed imported Gateway progress detaches by upstream operation even though the SSE stream remains open. Every retry and imported operation is canceled on shutdown.
  - A precommit hypothesis carries the provisional item ID that commit later promotes unchanged. The browser binds the first valid hypothesis for the active uncommitted take to that ID immediately, renders subsequent snapshots live, and requires the commit acknowledgment to confirm the same ID. Unknown hypotheses with no active take never mutate text.
- Rejected alternatives:
  - Keeping Workshop status frames, headers, guards, or types in Gateway because it preserves the forbidden product dependency.
  - Exposing the Gateway key to the webview because it expands browser credential exposure.
  - Adding STT wire types to `shared-protocol` because the relay does not parse payloads and shared fixtures provide sufficient Rust and TypeScript compatibility.
  - Merging FFI and safe backend code because it expands the unsafe-capable surface and obscures ownership.
  - Loading model copies per client because it violates the memory and worker-count constraints.
  - Using strong-reference counts for quiescence because unrelated references do not prove admitted work ownership.
  - Detaching native workers during profile replacement because live model memory and callbacks would outlast the generation while replacement proceeds.
  - Emitting standard item deltas before item creation because standard clients cannot reconcile an unpublished item.
  - Blocking or dropping an authoritative final segment on queue pressure because either can stall socket ownership or produce a false completed transcript.
  - Expanding algorithm scope during the boundary migration because structural and protocol changes need a stable behavioral baseline.
- Assumptions, risks, and notes:
  - The current packaged Whisper runtime, model pair, and native JFK fixture remain available for characterization. Missing native assets block equivalence claims.
  - Native model loading and decode may hang inside code Rust cannot preempt. Bounded control-plane response therefore sometimes requires refusing replacement or terminating the process rather than reclaiming the thread.
  - Reconstructing the old generation can fail after a determinate staged failure; this is reported as rollback failure and leaves speech unavailable.
  - OpenAI's Realtime schema can evolve. Canonical fixtures define the implemented subset, and compatibility claims must be rechecked when the upstream contract changes.
  - The custom hypothesis include value is intentionally outside official SDK closed enums and may require extension-aware client code.
  - A WebSocket server cannot prove peer receipt without acknowledgment. The no-loss guarantee covers internal accepted terminal events while the connection remains writable.
  - Manual microphone acceptance is a real release gate and cannot be replaced by synthetic audio alone.

## Project survey

- Build commands:
  - Prerequisite: Rust 1.89 or later and Node.js 22, then `npm ci --prefix crates/workshop-server/ui` and `npm ci --prefix crates/gateway-config-ui/ui` once per checkout.
  - `cargo build` builds the default workspace member, `gateway`, including the default config UI and STT features.
  - `cargo build -p workshop` builds the Tauri desktop product and its in-process `workshop-server`.
  - The two UI bundles build independently with `npm run build` in `crates/workshop-server/ui` and `crates/gateway-config-ui/ui`; Cargo build scripts place generated bundles in `OUT_DIR`.
- Focused test command patterns:
  - Rust unit or named test: `cargo test -p <crate> <test-name>`.
  - Rust integration harness: `cargo test -p <crate> --test it <test-name>`. Current Gateway, `gateway-stt`, and `workshop-server` integration suites use `tests/it/main.rs` as the harness and responsibility-named modules below it.
  - STT package gates: `cargo test -p gateway-transcribe`, `cargo test -p gateway-stt --test it`, `cargo test -p gateway-whisper-ffi`, `cargo test -p gateway`, and `cargo test -p workshop-server --test it`.
  - Native Whisper tests are ignored by default and use the same package command with `-- --ignored`; they require `PROMPTFORGE_WHISPER_LIBRARY` plus the gitignored `gateway-transcribe/tests/fixtures/ggml-tiny.en.bin` and `jfk.wav`, or `PROMPTFORGE_WHISPER_MODEL` and `PROMPTFORGE_WHISPER_AUDIO` overrides.
  - Workshop UI focused tests run directly from `crates/workshop-server/ui`, for example `node --test test/stt-stream.mjs`; the complete UI discovery command is the package's `npm test`.
  - Config UI focused tests run from `crates/gateway-config-ui/ui` with `node --test src/<area>.test.mjs`; `npm test` runs its complete discovered suite.
- Full-suite test commands:
  - Rust: `cargo test --workspace`. CI splits this into `cargo test --locked --workspace --exclude workshop --exclude workshop-server --all-features` on Linux and `cargo test --locked -p workshop -p workshop-server` on Windows.
  - Workshop UI: run `npm run typecheck`, then `npm run build`, then `npm test` as separate commands in `crates/workshop-server/ui`.
  - Config UI: run `npm run typecheck`, then `npm run build`, then `npm test` as separate commands in `crates/gateway-config-ui/ui`; its tests import the built `dist/app.js`, so build precedes test.
- Linter and formatter commands:
  - Rust formatting: `cargo fmt --all --check`.
  - Rust linting: `cargo clippy --workspace --all-targets --all-features -- -D warnings`; CI excludes `workshop` and `workshop-server` in the Linux job and lints those two packages on Windows.
  - Documentation gates: `cargo test --workspace --all-features --doc` and then `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features`.
  - Feature boundary gate: `cargo check -p gateway --no-default-features`.
  - Workshop UI layering and types: `npm run typecheck`, which runs `tsc --noEmit` and `check-layers.mjs`. Config UI uses `npm run typecheck` and runs `check-layers.mjs` through `npm test`. Neither UI package defines a standalone formatter command.
- Test placement and naming:
  - Rust unit tests are colocated in source modules under `#[cfg(test)]`; async tests use `#[tokio::test]`.
  - Cross-module and socket tests live under `tests/it/`, with shared fixtures in `tests/common/`. Test function names are lower snake case behavior statements.
  - Native and large-download tests are explicitly `#[ignore]` and name their required fixture or live dependency.
  - Workshop UI tests are either `ui/test/**/*.mjs` or colocated `ui/src/**/*.test.mjs`. Names are plain English behavior statements, and disposable-owning tests use `test/helpers/leak-check.mjs`.
- Directory map:
  - `.cargo/` contains repository Cargo configuration; `.github/` contains CI, release, nightly, native Whisper, and guide workflows plus reusable actions.
  - `crates/` is the product and library workspace. Current Gateway speech code is in `gateway-stt`, `gateway-transcribe`, and `gateway-whisper-ffi`; the planned `gateway-stt-engine` and `gateway-stt-backend-whisper` directories do not yet exist.
  - Gateway product crates are `gateway`, `gateway-config`, `gateway-config-ui`, `gateway-local`, `gateway-logging`, `gateway-routing`, `gateway-stt`, `gateway-transcribe`, `gateway-web-search`, and `gateway-whisper-ffi`.
  - Workshop product crates are `workshop` and `workshop-server`; the browser application is under `crates/workshop-server/ui`.
  - Cross-product substrate is in `shared-loopback`, `shared-progress`, `shared-protocol`, `shared-sidecar`, and the non-Rust `shared-ui` package.
  - PromptForge library crates use the `promptforge-*` prefix, while `build-*` crates are compile-time and CI tooling.
  - `design/` holds design material, `guide/` holds mdBook documentation, `images/` holds repository media, `prompts/` holds prompt programs, `tools/` holds repository tooling, and `vibe/` holds execution plans and `vibe/archdoc.md`.
- Current STT paths and component boundaries:
  - `crates/gateway/src/lib.rs` mounts authenticated `POST /v1/audio/transcriptions`, `/stt`, and `/stt/capability`; `crates/gateway/src/runner.rs` owns STT startup and profile-switch calls.
  - `crates/gateway-stt/src/runtime.rs` provisions artifacts and owns active engine publication, `src/api.rs` handles OpenAI multipart batch transcription, and the 850-line `src/stt.rs` owns the Workshop-specific streaming socket, take state, interim loop, finalization, and status frames. Its integration characterization is in `tests/it/stt.rs`.
  - `gateway-stt` currently depends on `gateway-transcribe`, `gateway-local`, `gateway-config`, `shared-progress`, and `workshop-server`. This is the current boundary to dismantle, not the target boundary already described above.
  - `crates/gateway-transcribe/src/` is the current engine package: `engine.rs` and `worker.rs` own model workers, `final_pass.rs` owns accurate-pass state, `segment.rs` owns segmentation, `prompt.rs` owns Whisper prompt fitting, `slot.rs` owns active engine publication, and `lib.rs` owns silence and window policy. It currently depends directly on `gateway-whisper-ffi`.
  - `crates/gateway-whisper-ffi/src/` is the runtime-loaded ABI leaf. `library.rs`, `context.rs`, `params.rs`, `raw.rs`, and `log.rs` contain the only STT native loading, pointer ownership, ABI layout, and unsafe calls.
  - `crates/gateway-config/src/config/stt.rs` owns STT model catalog entries and roles. Capture tuning still lives in `crates/gateway-config/src/config/workshop.rs` as `WorkshopSttConfig` under `[workshop.stt]`.
  - `crates/workshop-server/src/routes/stt.rs` currently proxies capability and relays `/stt`; `src/gateway.rs` owns the authenticated upstream socket connector. The relay currently parses private `workshop_status` frames rather than remaining payload opaque.
  - `crates/workshop-server/ui/src/ui/stt.ts` currently owns 16 kHz `f32` microphone capture, old start/stop framing, transcript insertion, and local capture errors; `ui/test/stt-stream.mjs` characterizes generation handling.
  - `vibe/archdoc.md` is the architecture anchor. It defines gateway, Workshop UI, executor, store, Lua boundary, and shared-substrate components, with dependencies flowing toward gateway, store, and shared substrate. Relevant invariants include gateway-only credential ownership, Workshop cross-site and WebSocket-origin rejection, descendant cancellation, service-owned connection records, explicit endpoint readiness, and config apply publication consistency.
- Visible conventions:
  - Crate prefixes encode product ownership. A shared dependency must live in a `shared-*` crate, and build-only tooling in `build-*`.
  - Cargo features gate real constraints only. Gateway `local`, `web-search`, `config-ui`, and `stt` features are additive and default on; the featureless Gateway check must remain green.
  - Rust modules are private by default with deliberate crate-root re-exports. Every public item requires rustdoc, public fields are avoided, libraries use typed errors, and behavior changes carry tests.
  - Runtime paths do not compile native code. Whisper is loaded from packaged runtime artifacts, worker threads own native contexts, and async callers exchange owned buffers through channels and oneshots.
  - Unsafe code, C symbols, ABI layouts, and raw Whisper pointers stay in `gateway-whisper-ffi`; each unsafe block has an adjacent `SAFETY` justification and pointers stay behind `Drop`-owning wrappers.
  - Workshop server route groups expose `routes(state) -> Router`; `app.rs` composes them. One task owns each ordinary socket, request/session errors are values, and in-process tests use `Router::oneshot` or spawn fixtures.
  - Workshop UI imports flow `ui -> services -> base`; `main.ts` is the composition root. The rule is enforced by `check-layers.mjs` during build, typecheck, and Cargo bundling.
  - Generated UI bundles are never checked in. `crates/workshop-server/module-ceilings.toml`, enforced by `cargo test -p workshop-server --test it`, is the only current source-module size ratchet.
- Rules manifest:
  - `AGENTS.md` governs the repository root.
  - `crates/gateway/AGENTS.md` governs `crates/gateway/`.
  - `crates/gateway-config/AGENTS.md` governs `crates/gateway-config/`.
  - `crates/gateway-local/AGENTS.md` governs `crates/gateway-local/`.
  - `crates/gateway-logging/AGENTS.md` governs `crates/gateway-logging/`.
  - `crates/gateway-routing/AGENTS.md` governs `crates/gateway-routing/`.
  - `crates/gateway-stt/AGENTS.md` governs `crates/gateway-stt/`.
  - `crates/gateway-transcribe/AGENTS.md` governs `crates/gateway-transcribe/`.
  - `crates/gateway-web-search/AGENTS.md` governs `crates/gateway-web-search/`.
  - `crates/gateway-whisper-ffi/AGENTS.md` governs `crates/gateway-whisper-ffi/`.
  - `crates/promptforge/AGENTS.md` governs `crates/promptforge/`.
  - `crates/promptforge-agent/AGENTS.md` governs `crates/promptforge-agent/`.
  - `crates/promptforge-core/AGENTS.md` governs `crates/promptforge-core/`.
  - `crates/promptforge-core-support/AGENTS.md` governs `crates/promptforge-core-support/`.
  - `crates/promptforge-lua/AGENTS.md` governs `crates/promptforge-lua/`.
  - `crates/promptforge-model-client/AGENTS.md` governs `crates/promptforge-model-client/`.
  - `crates/promptforge-parser/AGENTS.md` governs `crates/promptforge-parser/`.
  - `crates/promptforge-store/AGENTS.md` governs `crates/promptforge-store/`.
  - `crates/promptforge-tools/AGENTS.md` governs `crates/promptforge-tools/`.
  - `crates/promptforge-web-search/AGENTS.md` governs `crates/promptforge-web-search/`.
  - `crates/promptforge-webfetch/AGENTS.md` governs `crates/promptforge-webfetch/`.
  - `crates/shared-loopback/AGENTS.md` governs `crates/shared-loopback/`.
  - `crates/shared-progress/AGENTS.md` governs `crates/shared-progress/`.
  - `crates/shared-protocol/AGENTS.md` governs `crates/shared-protocol/`.
  - `crates/shared-sidecar/AGENTS.md` governs `crates/shared-sidecar/`.
  - `crates/shared-ui/AGENTS.md` governs `crates/shared-ui/`.
  - `crates/workshop/AGENTS.md` governs `crates/workshop/`.
  - `crates/workshop/icons/AGENTS.md` additionally governs `crates/workshop/icons/`.
  - `crates/workshop-server/AGENTS.md` governs `crates/workshop-server/`.
  - `crates/workshop-server/ui/AGENTS.md` additionally governs `crates/workshop-server/ui/`.

## Execution Instructions

Use Windows PowerShell 5.1. Every command below has an explicit working directory and runs separately, so no shell state or success chaining is assumed. Before Step 1, run `npm ci` separately in `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui` and `C:\Users\Vinnie\cursor\promptforge\crates\gateway-config-ui\ui`. Before native tests, extract `https://github.com/cppalliance/promptforge/releases/download/whisper-lib-b4938/whisper-b4938-windows-x86_64-cuda.zip` to `C:\Users\Vinnie\cursor\promptforge\local\stt-fixtures\`; place `ggml-tiny.en.bin` from `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin` and `jfk.wav` from `https://github.com/ggerganov/whisper.cpp/raw/master/samples/jfk.wav` in the current engine fixture directory; and let each native command set `$env:PATH` and `$env:PROMPTFORGE_WHISPER_LIBRARY` itself. Begin from a clean worktree and green baseline. Each step is one commit; formatting, warnings-denied linting for touched packages, and its listed commands must pass before the next step. When architecture changes invalidate an `AGENTS.md` rule, delete that text in the same commit and add only a minimal replacement when the new boundary would otherwise be unenforced. Do not restate root rules or this design plan in nested rule files.

The architecture harness enforces exact workspace-package edges across normal, development, and build dependencies, ignoring self-dependencies. Phase A, Steps 7 through 24: `gateway -> {gateway-config,gateway-config-ui,gateway-local,gateway-logging,gateway-routing,gateway-stt,gateway-web-search,promptforge-core,shared-loopback,shared-progress,shared-protocol,shared-sidecar}`; `gateway-stt -> {gateway-config,gateway-local,gateway-stt-backend-whisper,gateway-stt-engine,shared-progress,workshop-server}`; `gateway-stt-engine -> {}`; `gateway-stt-backend-whisper -> {gateway-stt-engine,gateway-whisper-ffi,shared-progress}`; `gateway-whisper-ffi -> {}`; `shared-loopback -> {}`; `workshop-server -> {build-ui,promptforge-agent,promptforge-core-support,promptforge-model-client,promptforge-store,promptforge-tools,shared-progress,shared-sidecar}`. Phase B, Steps 27 through 38, adds only `workshop-server -> shared-loopback`. Final Phase C, Step 39 onward, is Phase B with only `gateway-stt -> workshop-server` removed. No normal or development edge from `gateway` to `workshop-server`, and no new such edge from `gateway-stt`, may be added; the current `gateway-stt -> workshop-server` edge is solely a temporary removal target. Beginning with Step 7, every later step touching an STT source file, manifest, crate-root export, or ceiling runs `node tools/check-stt-architecture.mjs` followed by the unfiltered Rust architecture test.

### Step 1: Characterize current speech behavior [completed]

- Artifacts: split `crates/gateway-stt/tests/it/stt.rs` into `tests/it/batch.rs` and `tests/it/legacy_stream.rs`, extend `tests/common/mod.rs`, and register both modules in `tests/it/main.rs`.
- Scope: pin batch physical-model selection, current two-model streaming, policy constants, segment order, final authority, and cross-client failure behavior without changing production code.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it`
- Consumes and gates: consumes the green baseline; these assertions must be preserved by replacement fixtures before legacy tests retire.

### Step 2: Pin the pre-rename native target [completed]

- Artifacts: create `crates/gateway-transcribe/tests/native_whisper.rs` and preserve `tests/fixtures/ggml-tiny.en.bin`, `tests/fixtures/jfk.wav`, and their ignore rule.
- Scope: pin packaged-runtime loading, transcript text, decode policy, prompt behavior, and cleanup in one explicit ignored integration target.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `$fixture=(Resolve-Path 'local\stt-fixtures').Path; $env:PATH="$fixture;$env:PATH"; $env:PROMPTFORGE_WHISPER_LIBRARY=(Resolve-Path 'local\stt-fixtures\whisper.dll').Path; cargo test -p gateway-transcribe --test native_whisper -- --ignored`
- Consumes and gates: consumes Step 1 and the named external fixtures; the same assets and expected transcript gate Steps 4 and 6.

### Step 3: Freeze canonical Realtime fixtures [completed]

- Artifacts: create `crates/gateway-stt/tests/fixtures/realtime/*.json`, `tests/it/realtime_fixtures.rs`, and `crates/workshop-server/ui/test/realtime-wire-fixtures.mjs`; register `realtime_fixtures` in `crates/gateway-stt/tests/it/main.rs`.
- Scope: encode every event, effective session, error, usage, ID, hypothesis, and valid or invalid sequence from the Decision Record without mounting a route.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it realtime_fixtures`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `node --test test/realtime-wire-fixtures.mjs`
- Consumes and gates: consumes the complete 2026-09-05 wire contract; fixture parity gates every wire implementation and consumer.

### Step 4: Rename the engine without changing APIs [completed]

- Artifacts: rename `crates/gateway-transcribe/` to `crates/gateway-stt-engine/`; update root `Cargo.toml`, `Cargo.lock`, root `.gitignore`, the moved `AGENTS.md`, `crates/gateway-stt/Cargo.toml`, `crates/gateway-stt/AGENTS.md`, imports, and verified textual references in `tools/document.md`; do not touch `.github/workflows/whisper-lib.yml`, which has no crate reference.
- Scope: preserve behavior and current APIs, move fixtures and the existing engine rules with the crate, add no compatibility crate, and compile every current reverse consumer. This mechanical commit changes names only; Step 6 removes rules invalidated by the new boundary.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt-engine`
  - `C:\Users\Vinnie\cursor\promptforge`: `$fixture=(Resolve-Path 'local\stt-fixtures').Path; $env:PATH="$fixture;$env:PATH"; $env:PROMPTFORGE_WHISPER_LIBRARY=(Resolve-Path 'local\stt-fixtures\whisper.dll').Path; cargo test -p gateway-stt-engine --test native_whisper -- --ignored`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway`
- Consumes and gates: consumes Steps 1 and 2; all renamed consumers and the post-rename native target must pass in this commit.

### Step 5: Move take ownership into gateway-stt [completed]

- Artifacts: create `crates/gateway-stt/src/take.rs`, move segmentation and LocalAgreement state from `src/stt.rs` and `gateway-stt-engine/src/segment.rs` into gateway-stt modules, make `gateway-stt-engine/src/final_pass.rs` and `src/worker.rs` execute stateless decode jobs, and adapt the legacy stream in `gateway-stt/src/stt.rs` to the single `take::Take`.
- Scope: `Take` exclusively owns guidance, finalized history, segment aggregation, completion, and failure; remove engine reset channels and accumulated transcript state, create no engine `FinalTake`, and update every engine API consumer in the same commit.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt-engine`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it legacy_stream`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway`
- Consumes and gates: consumes characterization and the renamed engine; legacy ownership isolation gates Realtime reuse of `take.rs`.

### Step 6: Extract contracts and safe backend atomically [completed]

- Artifacts: create `gateway-stt-engine/src/decoder.rs` and `policy.rs`; create `crates/gateway-stt-backend-whisper/{Cargo.toml,AGENTS.md,src/lib.rs,src/config.rs,src/model.rs,src/prompt.rs,tests/native_whisper.rs}`; update root manifests, `gateway-stt` manifest and runtime, all imports, crate-root exports, `crates/gateway-stt/AGENTS.md`, and the moved `crates/gateway-stt-engine/AGENTS.md`.
- Scope: replace `EngineConfig` and constructors once, update every current gateway-stt and Gateway consumer in this commit, expose only the seven engine items and two backend items, and leave no FFI or prompt policy in the engine and no compatibility shim. Delete the moved engine rules that assign Whisper loading, prompt fitting, segmentation, take state, or FFI integration to the engine; retain only backend-neutral bounded-worker constraints. Reduce the service rules to facade, lifecycle, batch, Realtime, and sole take ownership. The new backend rule file contains only safe Whisper construction, prompt and decode policy, progress, and the prohibition on unsafe or host types.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt-engine`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt-backend-whisper`
  - `C:\Users\Vinnie\cursor\promptforge`: `$fixture=(Resolve-Path 'local\stt-fixtures').Path; $env:PATH="$fixture;$env:PATH"; $env:PROMPTFORGE_WHISPER_LIBRARY=(Resolve-Path 'local\stt-fixtures\whisper.dll').Path; cargo test -p gateway-stt-backend-whisper --test native_whisper -- --ignored`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-whisper-ffi`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway`
- Consumes and gates: consumes Step 5 stateless jobs; matching native output and green reverse consumers gate bounded workers.

### Step 7: Establish exact architecture ratchets [completed]

- Artifacts: create `tools/check-stt-architecture.mjs`; reduce `crates/gateway-stt/tests/it/architecture.rs` to Cargo metadata edge policy, strict ceiling and migration policy, and inherited lint checks; register it in `tests/it/main.rs`; create `module-ceilings.toml` in all four STT crates; remove the unused `syn` workspace and development dependencies; and add pinned tool installation plus both gates to the normal CI job.
- Scope: enforce the stated temporary and final workspace-edge allowlists through Cargo metadata, unsafe isolation through the existing compiler lint, production-library module cycles through filtered `cargo-modules` 0.25.0 DOT output collapsed to module nodes, effective public-root budgets through `cargo-public-api` 0.52.0 output, and current ceilings through strict policy files. The driver rejects wrong tool versions and malformed output. Temporary exceptions name their removal step. Do not retain source-level Rust syntax analysis.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo modules --version`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo public-api --version`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `node --test tools/check-stt-architecture.test.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes Step 6 final crate topology; the unfiltered command becomes mandatory after every later STT edit.

### Step 8: Bound workers and expose scripted tests [completed]

- Artifacts: revise engine `worker.rs`, `engine.rs`, `error.rs`, and manifest; add `test-fixtures` scripted `ModelFactory` and `Decoder`; forward test features in backend and `gateway-stt` manifests; add Gateway development wiring and `crates/gateway/src/test_support.rs` injection without a new production facade type.
- Scope: enforce `INTERIM_JOB_CAPACITY = 8` and `FINAL_JOB_CAPACITY = 8`, capacity and capacity-plus-one admission, cancellation, panic, factory failure, startup outcomes, cleanup, thread confinement, and non-detaching idempotent shutdown.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt-engine --features test-fixtures`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --features test-fixtures`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes Step 7; scripted injection gates deterministic lifecycle and socket tests without widening the six-type production facade.

### Step 9: Select and wire Miri [completed]

- Artifacts: create `.github/workflows/stt-miri.yml`, add Miri-safe pure worker tests under the engine `test-fixtures` feature, and document exclusions beside unsupported socket and FFI tests.
- Scope: pin `nightly-2026-09-05`, run only pure ownership and queue targets, and establish the repository-selected UB interpreter before service state exists.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `rustup toolchain install nightly-2026-09-05 --component miri`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo +nightly-2026-09-05 miri setup`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo +nightly-2026-09-05 miri test -p gateway-stt-engine --features test-fixtures miri_`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes Step 8 scripted workers; later pure service targets join this pinned workflow.

### Step 10: Migrate canonical configuration and every consumer [completed]

- Artifacts: replace `WorkshopSttConfig` with `SttPipelineConfig` across `gateway-config/src/config/{workshop.rs,stt.rs,tests.rs,tests/schema.rs,tests/serialize.rs,tests/validation.rs}`, `config.rs`, and `lib.rs`; update `gateway-stt/src/runtime.rs`; Gateway warnings and tests in `src/runner.rs`; `gateway.local.example.toml`; `crates/gateway-config/README.md`; `crates/gateway/README.md`; `crates/gateway/AGENTS.md`; config UI `services/config-store.ts`, `views/settings-view.ts`, `views/settings-sections.test.mjs`; source guides `guide/src/gateway/05-speech.md`, `guide/src/gateway/10-serving-and-observing.md`, `guide/src/workshop/01-application.md`, and `guide/src/workshop/07-voice.md`; generated `guide/src/SUMMARY.md`, `guide/src/gateway/index.md`, `guide/src/workshop/index.md`, `guide/promptforge-gateway-guide.md`, and `guide/promptforge-workshop-guide.md`.
- Scope: accept legacy `[workshop.stt]` only during parsing when `[stt]` is absent, reject both, serialize only `[stt]`, update all direct consumers in one commit, and provide no type or accessor alias. In `crates/gateway/AGENTS.md`, delete the stale statement that `[workshop.stt]` remains live and do not replace it with configuration detail already enforced by `gateway-config`.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-config`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway`
  - `C:\Users\Vinnie\cursor\promptforge\crates\gateway-config-ui\ui`: `npm run build`
  - `C:\Users\Vinnie\cursor\promptforge\crates\gateway-config-ui\ui`: `node --test src/views/settings-sections.test.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo run -p build-user-guide`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes Step 6 backend configuration; canonical schema and generated documentation gate the facade.

### Step 11: Add audio ingestion and shared PCM bytes [completed]

- Artifacts: add `base64 = "0.22"` to root `Cargo.toml` and `base64.workspace = true` to `crates/gateway-stt/Cargo.toml`; create `gateway-stt/src/audio.rs` and language-neutral `tests/fixtures/audio/pcm16le-24khz.json`; update ceilings.
- Scope: review Base64 license, Rust 1.89 support, and transitive tree before acceptance; implement endian decoding, Base64 boundaries, odd-byte state, continuous 24 kHz to 16 kHz conversion, flush, reset, durations, and size bounds.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install cargo-deny --locked`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo tree -p gateway-stt -i base64@0.22.1`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo deny check`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt audio`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes Step 10 tuning and Step 7 budgets; dependency review and byte fixtures gate Rust and JavaScript audio consumers.

### Step 12: Implement the private wire [completed]

- Artifacts: create `gateway-stt/src/realtime/{mod.rs,wire.rs,query.rs}`, bind them to canonical fixtures, update ceilings, and keep every type private.
- Scope: implement only the Decision Record subset, atomic updates, strict unknown-field rejection, opaque IDs, exact errors and usage, and query validation without opening a socket.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt realtime::wire`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it realtime_fixtures`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes Steps 3 and 11; exact fixture round trips gate session state.

### Step 13: Isolate architecture tools from ambient stable [completed]

- Artifacts: update `tools/check-stt-architecture.mjs`, `tools/check-stt-architecture.test.mjs`, and the architecture-tool setup in `.github/workflows/ci.yml`.
- Scope: install Rust 1.89 alongside the job's current stable toolchain, then make every `cargo-modules` 0.25.0 and `cargo-public-api` 0.52.0 child run with `RUSTUP_TOOLCHAIN=1.89` while leaving formatting, Clippy, tests, and documentation on stable. Preserve pinned versions and fail closed when Rust 1.89 or either tool is absent. Add child-environment tests proving ambient Cargo 1.98 cannot leak into architecture commands.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `node --test tools/check-stt-architecture.test.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `$env:RUSTUP_TOOLCHAIN='stable'; node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: this repairs the reproducible Linux CI failure where Cargo 1.98 rejects the pinned module tool's removed `--lockfile-path` metadata argument. It is independent of Realtime behavior and must pass before later steps rely on the architecture driver.

### Step 14: Stage Workshop sidecars in compile CI [completed]

- Artifacts: add `tools/stage-gateway-sidecar.mjs` and `tools/stage-gateway-sidecar.test.mjs`; update only the `check-workshop` and `check-workshop-linux` jobs in `.github/workflows/ci.yml`.
- Scope: before Workshop Clippy, tests, or build, compile `gateway` without default features and copy the real executable to `crates/workshop/binaries/promptforge-gateway-<target-triple><exe-suffix>` for the current Windows or Linux host. Remove the staged file after the Workshop commands. Keep the directory gitignored, reject missing or mismatched source binaries, and do not modify base Tauri configuration, release packaging, nightly packaging, or shipped sidecar features.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `node --test tools/stage-gateway-sidecar.test.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo build --locked -p gateway --no-default-features`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo check --locked -p workshop`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`
- Consumes and gates: this repairs the same missing `externalBin` failure observed as `promptforge-gateway-x86_64-pc-windows-msvc.exe` on Windows and `promptforge-gateway-x86_64-unknown-linux-gnu` on Linux. Target-mapping tests cover both hosts, and the existing CI clean-tree checks remain green.

### Step 15: Own sessions and uncommitted input [completed]

- Artifacts: create `gateway-stt/src/realtime/{session.rs,input.rs,registry.rs}`, `tests/it/realtime_session.rs`, register it in `tests/it/main.rs`, and update ceilings and Miri workflow filters.
- Scope: enforce `MAX_ACTIVE_REALTIME_SESSIONS = 8` with no wait queue and immediate ninth rejection, `SESSION_CANCEL_JOIN_CAPACITY = 8`, immutable first-append snapshots, clear, resampler reset, interim epochs, and capacity and capacity-plus-one tests.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it realtime_session`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo +nightly-2026-09-05 miri test -p gateway-stt --features test-fixtures miri_`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes scripted decoding, audio, wire, and the sole `take.rs`; snapshot and cancellation isolation gate commit.

### Step 16: Finalize committed items independently [completed]

- Artifacts: create `gateway-stt/src/realtime/{item.rs,result_mailbox.rs}`, extend `src/take.rs` and `tests/it/realtime_session.rs`, and update ceilings and Miri targets.
- Scope: enforce `MAX_COMMITTED_ITEMS_PER_SESSION = 4`, `SESSION_RESULT_CAPACITY = 16` plus one reserved terminal slot per item, one replaceable hypothesis slot per item, and `FINAL_SEGMENT_CAPACITY = 4` per item; add capacity and capacity-plus-one, durable lineage, reversed completion, saturated retry, pending failure, and one-terminal tests.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it realtime_session`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo +nightly-2026-09-05 miri test -p gateway-stt --features test-fixtures miri_`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes Step 15; complete item ownership gates facade replacement and generation quiescence.

### Step 17: Use preinstalled Rust on the native runner [completed]

- Artifacts: update only the `native-whisper` job in `.github/workflows/stt-miri.yml` and add `tools/check-stt-native-workflow.test.mjs`.
- Scope: remove `dtolnay/rust-toolchain@stable` from the self-hosted Windows job. Before Cargo caching or native tests, resolve the service account's existing `.cargo\bin`, require `rustup.exe` and `cargo.exe`, append that directory to `GITHUB_PATH`, and verify the preinstalled stable toolchain without installing rustup, creating proxy links, changing the default toolchain, or enabling self-update. Keep the hosted Linux Miri job and all native fixture or test commands unchanged.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `node --test tools/check-stt-native-workflow.test.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `$cargoBin=Join-Path $env:USERPROFILE '.cargo\bin'; $rustup=Join-Path $cargoBin 'rustup.exe'; $cargo=Join-Path $cargoBin 'cargo.exe'; if (-not (Test-Path $rustup -PathType Leaf) -or -not (Test-Path $cargo -PathType Leaf)) { throw 'self-hosted runner Rust is not provisioned' }; & $rustup toolchain list; & $cargo '+stable' '--version'`
- Consumes and gates: this repairs the self-hosted `NetworkService` failure where the toolchain action did not find the existing Cargo bin directory, attempted to reinstall rustup, and collided with an existing `rust-analyzer.exe`. The source test must prove the native job performs preflight before cache and contains no Rust installer action, while the hosted Miri job still installs its pinned nightly.

### Step 18: Replace runtime and route APIs atomically [completed]

- Artifacts: replace `gateway-stt/src/runtime.rs` with `service.rs`, `artifacts.rs`, `generation.rs`, `status.rs`, and `model.rs`; rename `api.rs` to `batch.rs`; replace `SttRuntime`, `SttState`, free route APIs, and old exports in `lib.rs`; update `gateway/src/{lib.rs,runner.rs,test_support.rs}` and all gateway-stt tests and common fixtures in the same commit.
- Scope: expose only `SpeechService` plus five supporting types, preserve batch and temporary legacy routes through methods, publish one complete snapshot, and retain test-only scripted construction behind `test-fixtures`.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --features test-fixtures`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo check -p gateway --no-default-features`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes Steps 10 and 16; every current reverse consumer compiles and tests in this API-changing commit.

### Step 19: Resolve Windows cache ownership by SID [completed]

- Artifacts: update `gateway-local/src/artifacts/confine.rs`, its focused tests under `gateway-local/src/artifacts/tests.rs`, and native STT test setup only if additional service-account assertions are required.
- Scope: replace environment-derived Windows account names with the current process SID from `whoami /user /fo csv /nh`. Parse exactly one account and canonical SID record, reject malformed or missing output, and grant `icacls` access to `*<SID>:(OI)(CI)F` before verifying that no broad principal remains. Preserve hidden-process flags, typed fail-closed errors, Unix mode enforcement, and ordinary interactive-user behavior.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-local`
  - `C:\Users\Vinnie\cursor\promptforge`: `$fixture=(Resolve-Path 'local\stt-fixtures').Path; $env:PATH="$fixture;$env:PATH"; $env:PROMPTFORGE_WHISPER_LIBRARY=(Resolve-Path 'local\stt-fixtures\whisper.dll').Path; $env:PROMPTFORGE_WHISPER_MODEL=(Resolve-Path 'local\stt-fixtures\ggml-tiny.en.bin').Path; $env:PROMPTFORGE_WHISPER_AUDIO=(Resolve-Path 'local\stt-fixtures\jfk.wav').Path; cargo test --locked -p gateway-stt --lib -- --ignored --test-threads=1`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo fmt --all --check`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo clippy -p gateway-local -p gateway-stt --all-targets --all-features -- -D warnings`
- Consumes and gates: this repairs native tests under the self-hosted Windows `NetworkService` account, where `WORKGROUP\<machine>$` cannot be mapped by `icacls`. Parser tests cover ordinary users, well-known service SIDs, malformed CSV, missing SID, command failure, and SID-prefix rendering. The real Windows DACL test and native STT targets must pass without changing runner identity or bypassing cache privacy.

### Step 20: Quiesce generations with explicit ownership [completed]

- Artifacts: extend `gateway-stt/src/{generation.rs,service.rs}`, create `replacement.rs`, create `tests/it/generation.rs`, register it in `tests/it/main.rs`, and update ceilings and Miri filters.
- Scope: serialize replacement, close admission, count requests and worker jobs, install fresh rollback epochs, drain without reference counts, reopen on deadline, and race replacement against shutdown.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it generation`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo +nightly-2026-09-05 miri test -p gateway-stt --features test-fixtures miri_`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes bounded jobs, committed items, and complete snapshots; bounded drain gates destructive staging.

### Step 21: Make profile replacement transactional [completed]

- Artifacts: complete `gateway-stt/src/{replacement.rs,artifacts.rs}`; update STT-only integration in `gateway/src/{runner.rs,config_apply.rs,config_pending.rs,config_write.rs,shutdown.rs}` and `gateway/tests/it/profiles.rs`.
- Scope: sync temporary persistence before replacement, stop old workers without detachment, stage under one deadline, publish after persistence, reconstruct on determinate failure, and invalidate tokens plus request controlled shutdown on fatal outcomes.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it generation`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway --test it profiles`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes Step 20; cancellation-at-every-await and rollback outcomes gate route mounting.

### Step 22: Separate origin predicates [completed]

- Artifacts: add named Gateway loopback-Origin and Workshop same-origin-authority predicates with predicate-only tests in `shared-loopback/src/lib.rs`; update `crates/shared-loopback/AGENTS.md`; do not mount sockets or change Workshop yet.
- Scope: cover absent native Origin, HTTP loopback forms, malformed, foreign, wrong-port, and mismatched authorities while keeping the two policies distinct. Remove rule text that describes the crate as Gateway-only or limited to two middlewares, then retain one concise rule that the Gateway and Workshop predicates are separately named, fail closed, and never share policy semantics.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p shared-loopback`
- Consumes and gates: consumes no route state; pure predicate behavior gates Gateway sockets and later Workshop manifest adoption.

### Step 23: Integrate generic speech facts [completed]

- Artifacts: update `gateway/src/{model_info.rs,system.rs,lib.rs}`, `gateway/tests/it/surface.rs`, and gateway-stt status and model modules.
- Scope: expose configured, ready, GPU, and generation status; advertise physical batch names and logical `realtime-transcribe` only when ready; omit speech without the feature.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway --test it surface`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo check -p gateway --no-default-features`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes Step 18 facade and Step 21 lifecycle; status correctness gates route publication.

### Step 24: Mount the additive Gateway route [completed]

- Artifacts: create `gateway-stt/src/realtime/route.rs`, update `realtime/mod.rs` and `service.rs`, mount it in `gateway/src/lib.rs`, create `gateway/tests/it/realtime_stt.rs`, and register it in `gateway/tests/it/main.rs`.
- Scope: add `WS /v1/realtime?intent=transcription` while retaining batch and legacy routes; test bearer, cookie, trusted-loopback, absent and hostile socket Origins, query conflicts, send deadlines, privacy, overload, and close 1012 through scripted decoders.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway --test it realtime_stt`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes Steps 12 through 23; the independent Gateway fixture path gates Workshop relay work.

### Step 25: Make session retirement verification event-driven [completed]

- Artifacts: update `gateway-stt/src/realtime/registry.rs`, its test-only facade as needed, `gateway-stt/tests/it/realtime_session.rs`, exact ceilings, and architecture policy.
- Scope: replace the fixed scheduler-yield budget used to observe retired session cleanup with an explicit notification emitted when registry-owned canceled tasks finish joining and admission is released. Await that signal under a real wall-clock deadline used only as a hang guard. Preserve production ownership, exact capacity, cancellation safety, immediate reuse after completed cleanup, and Miri-compatible pure state.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it dropping_session_retains_admission_until_interim_cleanup_joins`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it realtime_session`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo +nightly-2026-09-05 miri test -p gateway-stt --features test-fixtures miri_`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: this repairs the Linux CI failure where correct retirement did not finish within 1,000 scheduler yields. Tests must prove the waiter starts before release, cleanup wakes it exactly once, admission stays occupied until wakeup, and omitted cleanup reaches the bounded timeout.

### Step 26: Restore cross-platform Gateway warning cleanliness [completed]

- Artifacts: update only `gateway/build.rs`, `gateway/src/main.rs`, and focused source or compile tests when needed.
- Scope: compile the Windows application manifest constant only on Windows and apply the one-call unsafe-code lint expectation only when the Windows DPI-awareness block exists. Preserve Windows resources, process startup, lint policy, and every non-Windows code path; do not suppress warnings globally.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo check -p gateway`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo clippy -p gateway --all-targets --all-features -- -D warnings`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway`
- Consumes and gates: this repairs Linux warnings for unused `MANIFEST` and an unfulfilled `unsafe_code` expectation. Source checks must pin both declarations to Windows while existing Windows icon, manifest, and DPI tests remain green.

### Step 27: Add the Workshop relay beside legacy [completed]

- Artifacts: add `workshop-server/src/routes/realtime.rs`, a separate Realtime connector in `src/gateway.rs`, route composition in `src/routes.rs` and `src/app.rs`, `shared-loopback.workspace = true` in `workshop-server/Cargo.toml`, `tests/it/realtime_relay.rs`, and its registration in `tests/it/main.rs`.
- Scope: retain `routes/stt.rs`, old connector, status parsing, old UI, and every old test; the new relay fixes the upstream target, attaches the bearer, stays payload-opaque, and preserves type, close, ping, pong, origin, and subprotocol semantics.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p workshop-server --test it realtime_relay`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p workshop-server --test it stt`
- Consumes and gates: consumes Step 22 Workshop predicate and Step 24 public fixtures, but adds no dependency on Gateway or gateway-stt.

### Step 28: Prove the actual worklet bytes [completed]

- Artifacts: revise `workshop-server/ui/pcm-worklet.js`, create `ui/src/services/speech-capture.ts`, create `ui/test/pcm-worklet.mjs`, and consume `gateway-stt/tests/fixtures/audio/pcm16le-24khz.json`.
- Scope: make the dedicated JavaScript harness load the real worklet in a processor shim and assert little-endian bytes, clipping, transferred `ArrayBuffer` type, partial-buffer carry, and 24 kHz output; `stt-stream.mjs` is not evidence for worklet encoding.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `node --test test/pcm-worklet.mjs`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm run typecheck`
- Consumes and gates: consumes Step 11 language-neutral bytes and Step 27 additive relay; byte parity gates browser migration.

### Step 29: Migrate Workshop browser speech [completed]

- Artifacts: create `workshop-server/ui/src/services/realtime-transcription.ts`; update `src/ui/stt.ts`, `src/ui/prompt-input.ts`, and `src/main.ts`; replace assertions in `test/agent-stt.mjs`, `agent-stt-boot.mjs`, and `stt-stream.mjs`; retain server legacy seams and `test/stt-capability.mjs`.
- Scope: switch the browser to Realtime, hypothesis replacement, authoritative completion, local status, second take, clear, overlapping items, and recoverable errors while the server fallback remains removable only after physical acceptance.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm run build`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `node --test test/agent-stt.mjs test/agent-stt-boot.mjs test/stt-stream.mjs test/realtime-wire-fixtures.mjs test/pcm-worklet.mjs`
- Consumes and gates: consumes Steps 3, 27, and 28; browser acceptance gates independent full-path automation.

### Step 30: Prove both fixture-driven halves [completed]

- Artifacts: extend `gateway/tests/it/realtime_stt.rs`, `workshop-server/tests/it/realtime_relay.rs`, and Workshop UI sequence fixtures; add no dual-server Gateway test and no cross-product development dependency.
- Scope: Gateway independently drives canonical sequences through scripted decoders; Workshop independently drives the same sequences through a fake upstream and fake media; only installed-package acceptance claims the real dual-server path.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway --test it realtime_stt`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p workshop-server --test it realtime_relay`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm test`
- Consumes and gates: consumes Steps 24 through 29; both independent halves must pass before packaging.

### Step 31: Prevent silent model turns without selection [completed]

- Artifacts: update `workshop-server/ui/src/ui/agent-session-view.ts`, focused UI tests, `promptforge-agent` model-call error observation, the built-in `workshop-server/agents/chat.lua` only if needed to preserve recoverable looping, and Workshop agent integration tests.
- Scope: when `ModelService.current` is empty, keep input text intact, prevent `AgentSessionService.respond`, disable or reject every click and keyboard submission path, and show a local `Select a model before sending.` status. Subscribe to model-selection changes so submission becomes available immediately after a valid selection. If a selected binding disappears between submission and `models.chat`, emit `ModelTurnFailed` through the existing observer before returning the error to Lua; the built-in `pcall` may then continue to the next `user_input` without swallowing operator-visible failure.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p promptforge-agent`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p workshop-server --test it chat_gate`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `node --test test/agent-stt.mjs test/agent-stt-boot.mjs test/prompt-input.mjs`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm run typecheck`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm run build`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm test`
- Consumes and gates: this repairs the installed-package observation where the model picker still showed `Select model`, the session persisted a user-input tool result with no assistant event, Gateway received no model request, and Lua returned silently to input. Tests must cover click and keyboard submission, selection arrival, selection loss after submission, one visible error, retained text, and a successful next turn.

### Step 32: Converge Workshop state after simultaneous startup [completed]

- Artifacts: update `workshop-server/src/heartbeat.rs`, `workshop-server/src/gateway_progress.rs`, shared progress import support only if needed, `workshop-server/ui/src/services/realtime-transcription.ts`, and focused Rust and UI lifecycle tests.
- Scope: when Gateway health stays reachable but its first profile or model refresh was empty, retry catalog and profile refresh under the existing bounded heartbeat cadence until a selectable model is retained, then restore selection exactly once. Reconnect a failed initial Realtime socket under bounded cancel-safe backoff without requiring repeated microphone clicks. Track imported Gateway progress by upstream operation ID and detach an operation when its root finishes while keeping the never-ending SSE subscription alive, so the status renderer clears progress and restores LEDs. Cancel refresh, reconnect, and progress ownership on shutdown or disposal.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p workshop-server heartbeat`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p workshop-server gateway_progress`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `node --test test/speech-capture.mjs test/agent-stt-boot.mjs`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm run typecheck`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm run build`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm test`
- Consumes and gates: this repairs the installed observation where Gateway became ready in under two seconds but Workshop retained an empty model picker, Realtime remained connecting for 20 to 30 seconds, and a completed profile operation left the progress bar visible instead of restoring LEDs. Tests must keep health continuously true while catalog readiness changes, keep the progress SSE open after root completion, and force Realtime reconnect cancellation.

### Step 33: Bind live hypotheses before commit acknowledgment [completed]

- Artifacts: update `workshop-server/ui/src/ui/realtime-stt.ts`, its service only if typed provisional-item state is needed, and focused browser speech tests.
- Scope: when a valid hypothesis arrives for an unknown item while exactly one active uncommitted take exists, bind that provisional item ID to the take before applying the snapshot. Require the later `input_audio_buffer.committed` acknowledgment to name the same item, preserve FIFO tombstones and overlapping committed items, and ignore unknown hypotheses when no active take exists. Render every revision as replacement text while recording continues, then preserve authoritative completion behavior.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `node --test test/agent-stt.mjs test/stt-stream.mjs`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm run typecheck`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm run build`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm test`
- Consumes and gates: this repairs the installed observation where correct final text appeared only after stop because every precommit hypothesis was ignored until commit assigned the take's item ID. Tests must force multiple revisions before acknowledgment, mismatched acknowledgment, no-active-take input, overlap, clear, cancellation, and final replacement.

### Step 34: Converge running chat sessions with the live model catalog [completed]

- Artifacts: update `crates/workshop-server/src/session_agents.rs`, catalog and menu predicates only where needed, and `crates/workshop-server/tests/it/chat_gate.rs`.
- Scope: prevent an auto-launched built-in chat session from freezing an empty or obsolete model catalog while the Gateway profile is still loading. Keep model bindings frozen within one agent run, but make catalog generation part of the Workshop supervisor lifecycle: wait for at least one chat-capable model before starting a run, and safely relaunch over the retained event log when the chat catalog generation changes. Use one chat-capable predicate for menu readiness, picker restoration, and agent catalog construction so transcription-only entries never advertise chat readiness. Preserve cancellation, retained history, profile switching, and one visible recoverable failure if a selected binding disappears during dispatch.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p workshop-server --test it chat_gate`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p workshop-server session_agents`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo clippy -p workshop-server --all-targets --all-features -- -D warnings`
- Consumes and gates: this repairs the installed race where chat auto-launched about one second before Gateway published `claude-opus-4-6`; the picker later converged but the running session retained an empty model catalog and failed locally before any Gateway request. Tests must launch chat against an empty catalog, publish and select a chat model later, prove one completion request, replace the catalog during a profile switch, and reject transcription-only readiness.

### Step 35: Compose each live hypothesis from disjoint transcript ownership [completed]

- Artifacts: update `crates/gateway-stt/src/session.rs`, `crates/gateway-stt/src/realtime/server.rs`, the engine interim snapshot type and assembly only where ownership requires it, canonical wire fixtures, Gateway Realtime route tests, and the focused Workshop browser replay.
- Scope: return one coherent interim snapshot whose finalized, agreed, and tentative fields are disjoint and own their exact boundary whitespace. Serialize visible `transcript` from that snapshot exactly once. Do not independently prepend `Take::finalized` to cumulative committed text, and do not read finalized state twice while assembling one event. Preserve provisional promotion, divergent final reconciliation, authoritative completion, fallback, and item ordering.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway --test it realtime_stt`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `node --test test/agent-stt.mjs test/stt-stream.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo clippy -p gateway-stt -p gateway --all-targets --all-features -- -D warnings`
- Consumes and gates: this repairs installed live revisions that repeated prior speech and lost spaces until Stop replaced them with the authoritative final. Tests must finalize speech with closing silence, append later speech, assert no duplicated prefix, cover nonempty finalized, agreed, and tentative fields with exact spaces, reconcile a divergent provisional prefix, serialize producer-generated canonical snapshots, and replay them through the browser replacement path.

### Step 36: Schedule and rebase native whole-window hypotheses [completed]

- Artifacts: update `crates/gateway-stt/src/realtime/route.rs`, the session-owned Realtime task and lifecycle modules, `crates/gateway-stt/src/take/interim.rs`, interim request or snapshot metadata, scripted decoder controls, Gateway Realtime route tests, and one ignored packaged-native Realtime test.
- Scope: remove synchronous interim decoding from each 100 ms append. Use the active `EnginePolicy` interval and minimum window, skip silent windows, permit one interim decode in flight per session, and coalesce appends to the newest eligible audio snapshot. Carry the decoded window's start and end sample offsets with its whole-window transcript. Treat native interim output as replacement text for that audio window, not an incremental suffix: revisions at one origin replace prior provisional text, a consumed segment boundary starts a new provisional region even while finalization is pending, and an advancing window origin rebases through explicit overlap without retaining a divergent prefix twice. Keep finalized text authoritative, preserve ordered completion and cancellation, and report `audio_start_ms` and `audio_end_ms` from the accepted snapshot rather than hard-coding zero.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway --test it realtime_stt`
  - `C:\Users\Vinnie\cursor\promptforge`: `$fixture=(Resolve-Path 'local\stt-fixtures').Path; $env:PATH="$fixture;$env:PATH"; $env:PROMPTFORGE_WHISPER_LIBRARY=(Resolve-Path 'local\stt-fixtures\whisper.dll').Path; cargo test -p gateway --test it realtime_stt_native_incremental -- --ignored`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo clippy -p gateway-stt -p gateway --all-targets --all-features -- -D warnings`
- Consumes and gates: this repairs the post-Step 35 installed failure where producer fields were string-disjoint but still represented overlapping audio. Tests must prove no decode before 500 ms, silence suppression, one in-flight decode with newest-snapshot coalescing, replacement of `"Why is it"` by revised `"Why is this"`, a delayed-finalization segment boundary, a tiny sliding window with advancing audio offsets and no repeated overlap, cancellation cleanup, and incrementally growing then sliding packaged-native JFK audio.

### Step 37: Reconcile explicitly skipped final ranges [completed]

- Artifacts: update `crates/gateway-stt/src/segment.rs`, finalization command and outcome types, `TakeState` finalized coverage, `WholeWindowState` accepted snapshot coverage, Realtime completion assembly, scripted fixtures, and focused Gateway STT and Realtime tests.
- Scope: distinguish an intentionally declined final range from a decoded empty transcript. Track the latest accepted hypothesis text with its exact committed audio coverage through sealing. Keep every nonempty or genuinely decoded final result authoritative for the range it processed, but conservatively fill only ranges the final path explicitly skipped because they were below the speech-segment threshold, below the final minimum window, or silent. Do not advance decoded-final coverage for a skipped range until it is reconciled. Never carry text beyond committed audio, reuse stale snapshot text, preserve a provisional branch over a divergent nonempty final, or turn pure silence without an accepted hypothesis into text.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway --test it realtime_stt`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt take`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo clippy -p gateway-stt -p gateway --all-targets --all-features -- -D warnings`
- Consumes and gates: this repairs the post-Step 36 installed failure where a correct final hypothesis word vanished on Stop. Tests must combine an accepted hypothesis with stop-flush silence that closes 300 ms speech into a skipped sub-500 ms final segment, repeat for a sub-250 ms click-consumed region, prove divergent nonempty final text overrides provisional text, keep pure silence empty, and reject stale or beyond-commit hypothesis coverage.

### Step 38: Pass installed Windows microphone acceptance [completed]

- Artifacts: stage `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`, build `target/release/bundle/nsis/*-setup.exe`, install `promptforge-workshop.exe` and its sibling `promptforge-gateway.exe`, and create `design/generic-realtime-stt-acceptance.md`.
- Scope: follow `.github/workflows/release-workshop.yml` sidecar staging and Windows installer layout, but build a local unsigned NSIS package by passing `{"bundle":{"createUpdaterArtifacts":false}}` only through the Tauri command-line configuration override. Do not modify `tauri.conf.json`, release workflows, updater settings, or signing behavior. Install the resulting package, verify its sibling binaries and hashes, and record microphone revision, completion, second take, clear, cancellation, and recoverable permission or device failure with timestamps. State explicitly that signing was not tested.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
  - `C:\Users\Vinnie\cursor\promptforge`: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
  - `C:\Users\Vinnie\cursor\promptforge`: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop`: `$env:RUSTUP_TOOLCHAIN='stable'; cargo tauri build --bundles nsis --config '{"bundle":{"createUpdaterArtifacts":false}}'`
  - `C:\Users\Vinnie\cursor\promptforge`: `$setup=Get-ChildItem -Recurse 'target\release\bundle\nsis' -Filter '*-setup.exe' | Select-Object -First 1; if (-not $setup) { throw 'no NSIS installer' }; Start-Process $setup.FullName -ArgumentList '/S' -Wait`
  - `C:\Users\Vinnie\cursor\promptforge`: `$workshop=@($env:LOCALAPPDATA,$env:ProgramFiles,${env:ProgramFiles(x86)}) | ForEach-Object { Get-ChildItem $_ -Recurse -Filter 'promptforge-workshop.exe' -ErrorAction SilentlyContinue } | Select-Object -First 1; $gateway=@($env:LOCALAPPDATA,$env:ProgramFiles,${env:ProgramFiles(x86)}) | ForEach-Object { Get-ChildItem $_ -Recurse -Filter 'promptforge-gateway.exe' -ErrorAction SilentlyContinue } | Select-Object -First 1; if (-not $workshop -or -not $gateway) { throw 'installed Workshop or Gateway missing' }; Start-Process $workshop.FullName`
- Consumes and gates: consumes Steps 30 through 37; the operator must exercise the installed application and record passing evidence, which alone gates legacy removal.

### Step 39: Remove legacy seams and tests [completed]

- Artifacts: remove gateway-stt legacy route/status code from `src/stt.rs` after moving retained `Take` behavior, remove old exports and Gateway mounts, remove Workshop `routes/stt.rs`, old connector/status parsing and capability proxy, update route composition, remove `workshop-server` from `gateway-stt/Cargo.toml`, update `Cargo.lock`, update `crates/gateway/AGENTS.md` and `crates/workshop-server/AGENTS.md`, and retire `tests/it/legacy_stream.rs`, old `tests/it/stt.rs` registrations, and UI `test/stt-capability.mjs`.
- Scope: map every retired legacy assertion to Steps 3, 24, 27, 29, and 30 replacement evidence in the commit rationale; deletion is justified only because those fixtures preserve the behavior, and the final allowlist, zero legacy symbols, and only batch plus Realtime routes are enforced. Delete Gateway's temporary `gateway-stt -> workshop-server` exception and Workshop's `spawn_with_routes`, Gateway-owned socket attachment, status-bus, and Whisper-job language. Add only one Workshop-local rule if needed: its Realtime relay authenticates upstream, validates browser origin, and never parses speech payloads or owns speech state.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p workshop-server --test it`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm test`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo check -p gateway --no-default-features`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
- Consumes and gates: consumes the passing installed-package record; failed replacement coverage blocks deletion.

### Step 40: Finalize architecture and documentation [completed]

- Artifacts: finalize the architecture harness and all four ceilings; audit root `AGENTS.md` plus the targeted `gateway-stt`, `gateway-stt-engine`, `gateway-stt-backend-whisper`, `gateway`, `shared-loopback`, and `workshop-server` rule files for stale pre-migration text; update `.github/workflows/ci.yml`, `.github/workflows/stt-miri.yml`, `crates/gateway/README.md`, `crates/gateway-config/README.md`, `crates/workshop-server/README.md`, source guides `guide/src/gateway/05-speech.md`, `guide/src/gateway/10-serving-and-observing.md`, `guide/src/workshop/01-application.md`, and `guide/src/workshop/07-voice.md`; generated `guide/src/SUMMARY.md`, `guide/src/gateway/index.md`, `guide/src/workshop/index.md`, `guide/src/language/index.md`, `guide/src/agent/index.md`, `guide/promptforge-gateway-guide.md`, `guide/promptforge-workshop-guide.md`, `guide/promptforge-language-guide.md`, and `guide/promptforge-agent-guide.md`; `design/generic-realtime-stt.md`; and `design/generic-realtime-stt-acceptance.md`.
- Scope: remove temporary allowlist edges, enforce exact final dependencies, cycles, public counts, every 500-line ceiling, Gateway-only build isolation, normal-CI scripted coverage, and before or after debt counts. The rules audit deletes obsolete or duplicated lines first and edits only files with a concrete contradiction. Keep root `AGENTS.md`, `gateway-whisper-ffi/AGENTS.md`, `gateway-config/AGENTS.md`, and `workshop-server/ui/AGENTS.md` unchanged unless the final implementation proves one of their current constraints false.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo run -p build-user-guide`
  - `C:\Users\Vinnie\cursor\promptforge`: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install mdbook --version 0.4.44 --locked`
  - `C:\Users\Vinnie\cursor\promptforge`: `mdbook build guide`
- Consumes and gates: consumes Step 39 final topology; final verification starts only with zero temporary exceptions.

### Step 41: Bookend Gateway serving logs [completed]

- Artifacts: update only `crates/gateway/src/main.rs`, `crates/gateway/tests/it/boot.rs`, and this step's active-plan bookkeeping.
- Scope: in `init_logging()`, immediately after installing the subscriber with the file layer, emit the first serving-run file record as `promptforge-gateway {version} starting` before the existing `logging to {path}` record. After the serving result determines success or failure and before `LogRuntime::shutdown`, emit `gateway exiting` on success or `gateway exiting after a fatal error` after `log_error_chain` on failure. Emit terminal records only when file logging initialized. Preserve no-subscriber behavior for help, version, diagnostics, second-instance handoff, and stdout-only fallback. Do not modify `gateway-logging`, CLI parsing, queues, sinks, retention, rotation, redaction, or subscriber ownership.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo fmt --all --check`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo clippy -p gateway --all-targets --all-features -- -D warnings`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway`
- Consumes and gates: this operator-requested observability correction is independent of STT and follows Step 40 only to preserve a single ordered run. Extend the existing child-process log tests so a serving log's first line contains the versioned launch record, normal route shutdown leaves the clean terminal record last, and fatal-chain logging places the fatal terminal record after the complete chain. Existing no-log and no-rotation tests must remain unchanged and green. Step 42's full release verification must pass after this change.

### Step 42: Run every release gate and repeat acceptance [completed]

- Artifacts: append command results, hashes, ratchet counts, native equivalence, generated-doc cleanliness, and repeated installed-microphone evidence to `design/generic-realtime-stt-acceptance.md`; if the installed pair exposes a release-blocking defect, repair it in this still-provisional final commit and repeat every affected gate.
- Scope: run every exit criterion independently under PowerShell 5.1 and stop on any failure. The final installed run exposed one such defect: after a local sidecar Gateway exits, Workshop keeps a dead random-port endpoint forever. Add local-sidecar supervision in `crates/workshop`, a replaceable endpoint and credentials snapshot shared by every `workshop-server` Gateway client path, and bounded connection-file re-resolution plus sibling relaunch. Identify a replacement by a new PID or boot identity, validate its live process image, health, and bearer acceptance, then atomically publish the exact endpoint and credential pair from its connection file before heartbeat, progress, catalog, chat, proxy, and Realtime retries resume. `[server].api_key` is a long-term configured credential: generate it only when creating a missing default config, preserve it when the config is unchanged, and propagate a configured edit atomically after restart. The OS may reuse a port, so neither the port nor credential must differ across a valid replacement. A browser Realtime retry after replacement must reach ready without reloading Workshop. Never relaunch or mutate an explicitly configured LAN Gateway, never expose bearer keys, and preserve the Step 41 logging scope without adding shutdown-source records.
- Focused test commands:
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo fmt --all --check`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test --workspace`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test --workspace --all-features --doc`
  - `C:\Users\Vinnie\cursor\promptforge`: `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo deny check`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo build -p gateway`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo build -p workshop`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo check -p gateway --no-default-features`
  - `C:\Users\Vinnie\cursor\promptforge`: `$fixture=(Resolve-Path 'local\stt-fixtures').Path; $env:PATH="$fixture;$env:PATH"; $env:PROMPTFORGE_WHISPER_LIBRARY=(Resolve-Path 'local\stt-fixtures\whisper.dll').Path; cargo test -p gateway-stt-backend-whisper --test native_whisper -- --ignored`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo +nightly-2026-09-05 miri test -p gateway-stt-engine --features test-fixtures miri_`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo +nightly-2026-09-05 miri test -p gateway-stt --features test-fixtures miri_`
  - `C:\Users\Vinnie\cursor\promptforge`: `node tools/check-stt-architecture.mjs`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo test -p gateway-stt --test it architecture`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm run typecheck`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm run build`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\ui`: `npm test`
  - `C:\Users\Vinnie\cursor\promptforge\crates\gateway-config-ui\ui`: `npm run typecheck`
  - `C:\Users\Vinnie\cursor\promptforge\crates\gateway-config-ui\ui`: `npm run build`
  - `C:\Users\Vinnie\cursor\promptforge\crates\gateway-config-ui\ui`: `npm test`
  - `C:\Users\Vinnie\cursor\promptforge`: `cargo run -p build-user-guide`
  - `C:\Users\Vinnie\cursor\promptforge`: `mdbook build guide`
  - `C:\Users\Vinnie\cursor\promptforge`: `git diff --exit-code -- guide/src/SUMMARY.md guide/src/gateway/index.md guide/src/workshop/index.md guide/src/language/index.md guide/src/agent/index.md guide/promptforge-gateway-guide.md guide/promptforge-workshop-guide.md guide/promptforge-language-guide.md guide/promptforge-agent-guide.md`
  - `C:\Users\Vinnie\cursor\promptforge`: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
  - `C:\Users\Vinnie\cursor\promptforge`: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
  - `C:\Users\Vinnie\cursor\promptforge`: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
  - `C:\Users\Vinnie\cursor\promptforge\crates\workshop`: `$env:RUSTUP_TOOLCHAIN='stable'; cargo tauri build --bundles nsis --config '{"bundle":{"createUpdaterArtifacts":false}}'`
  - `C:\Users\Vinnie\cursor\promptforge`: `$setup=Get-ChildItem -Recurse 'target\release\bundle\nsis' -Filter '*-setup.exe' | Select-Object -First 1; if (-not $setup) { throw 'no NSIS installer' }; Start-Process $setup.FullName -ArgumentList '/S' -Wait`
  - `C:\Users\Vinnie\cursor\promptforge`: `$workshop=@($env:LOCALAPPDATA,$env:ProgramFiles,${env:ProgramFiles(x86)}) | ForEach-Object { Get-ChildItem $_ -Recurse -Filter 'promptforge-workshop.exe' -ErrorAction SilentlyContinue } | Select-Object -First 1; $gateway=@($env:LOCALAPPDATA,$env:ProgramFiles,${env:ProgramFiles(x86)}) | ForEach-Object { Get-ChildItem $_ -Recurse -Filter 'promptforge-gateway.exe' -ErrorAction SilentlyContinue } | Select-Object -First 1; if (-not $workshop -or -not $gateway) { throw 'installed Workshop or Gateway missing' }; Start-Process $workshop.FullName`
- Consumes and gates: consumes Step 41, then repeats the Step 38 installed-package microphone scenarios. In addition to every listed command, tests must keep the installed local pair alive beyond 60 seconds, terminate the local Gateway while Workshop remains open, prove one bounded relaunch publishes a validated new process or boot identity with the exact connection-file endpoint and credential, and prove health, model catalog, chat, progress, config proxy, and Realtime recover against that replacement. Deterministic coverage must include a same-port and same-key replacement identity plus a configured key change propagated after restart. Explicit LAN configuration must stay fixed and unrelaunched. Completion requires every command, ratchet, generated-doc check, and physical scenario to pass with no open finding.

Stop and revise after repeated same-signature failures, an upstream wire incompatibility, an unacceptable dependency license, Rust 1.89 incompatibility, unsafe or transitive expansion, or evidence that two generations cannot satisfy memory constraints. Do not modify Gateway CLI, diagnostics, logging queues, sinks, rotation, logging lifecycle beyond Step 41's exact two records, or completed Workshop baseline-ratchet work. VAD tuning, decode-quality changes, native streaming, batching, denoising, a new WER harness, dynamic plugins, a fifth STT crate, shared STT wire types, browser credentials, WebRTC, and automatic turn detection remain outside this plan.