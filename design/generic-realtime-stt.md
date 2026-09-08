# Generic Realtime speech-to-text architecture

## Outcome

PromptForge exposes speech as a Gateway product capability. The Gateway owns speech artifacts, models, workers, profile replacement, batch transcription, and Realtime transcription. Workshop is an independent consumer: its server authenticates and relays one fixed Realtime target, and its browser UI owns microphone capture and transcript presentation.

The only live streaming endpoint is `WS /v1/realtime?intent=transcription`. The removed `/stt` and `/stt/capability` routes, Workshop-specific status frames, and custom status headers have no compatibility path.

## Component boundaries

- `gateway` owns route mounting, authentication, profile-switch transactions, model discovery, and operational status.
- `gateway-stt` is the cloneable speech facade. It owns artifact preparation, complete generation snapshots, batch and Realtime routes, session and item orchestration, take state, segmentation, hypothesis agreement, transcript aggregation, and wire translation.
- `gateway-stt-engine` owns backend-neutral decoder contracts, stateless decode jobs, bounded serialized workers, startup deadlines, cancellation observation, and joined shutdown.
- `gateway-stt-backend-whisper` owns safe Whisper construction, checked configuration, prompt fitting, decode parameters, native-load progress, and backend error translation.
- `gateway-whisper-ffi` is the only unsafe STT crate. It owns runtime-loaded C symbols, ABI layouts, native pointers, and their lifetimes.
- `shared-loopback` owns distinct Gateway loopback-Origin and Workshop same-origin-authority policies.
- `workshop-server` owns only the authenticated, payload-opaque Realtime relay. The Workshop UI owns capture, connection recovery, hypothesis replacement, and user-visible dictation status.

The core direction is:

```text
gateway -> gateway-stt -> gateway-stt-engine
                    |
                    +-> gateway-stt-backend-whisper
                              |
                              +-> gateway-stt-engine
                              +-> gateway-whisper-ffi

workshop-server -> shared-loopback
```

No Gateway STT crate depends on Workshop. No Workshop crate depends on a Gateway STT implementation crate.

## Exact workspace dependency policy

- `gateway` -> `gateway-config`, `gateway-config-ui`, `gateway-local`, `gateway-logging`, `gateway-routing`, `gateway-stt`, `gateway-web-search`, `promptforge-core`, `shared-loopback`, `shared-progress`, `shared-protocol`, `shared-sidecar`
- `gateway-stt` -> `gateway-config`, `gateway-local`, `gateway-stt-backend-whisper`, `gateway-stt-engine`, `shared-progress`
- `gateway-stt-engine` -> none
- `gateway-stt-backend-whisper` -> `gateway-stt-engine`, `gateway-whisper-ffi`, `shared-progress`
- `gateway-whisper-ffi` -> none
- `shared-loopback` -> none
- `workshop-server` -> `build-ui`, `promptforge-agent`, `promptforge-core-support`, `promptforge-model-client`, `promptforge-store`, `promptforge-tools`, `shared-loopback`, `shared-progress`, `shared-sidecar`

The architecture test reads Cargo metadata across normal, development, target-specific, and build dependencies. Any extra or missing workspace edge fails.

## Public surfaces

The final effective crate-root counts are exact:

- `gateway-stt`: 6
- `gateway-stt-engine`: 7
- `gateway-stt-backend-whisper`: 2
- `gateway-whisper-ffi`: 6

`gateway-stt` exposes the lifecycle facade and opaque supporting facts, not route handlers, wire types, workers, sessions, or takes. The engine exposes only backend-neutral contracts. The safe Whisper backend exposes only its backend and checked configuration.

Active physical speech model names remain batch selectors. `realtime-transcribe` is a reserved logical name advertised only while a complete interim and final generation is ready. Generic Gateway status reports `configured`, `ready`, `gpu`, and `generation`; builds without STT omit speech status.

## Realtime contract

The request query must be exactly `intent=transcription`. Missing, duplicate, malformed, unsupported, or unknown parameters are rejected before upgrade. Gateway authentication runs before the session. A native client may omit Origin; a browser Origin must be HTTP loopback. Workshop separately requires browser Origin authority to match the request authority and constructs the fixed authenticated upstream target itself.

The supported client events are `session.update`, `input_audio_buffer.append`, `input_audio_buffer.clear`, and `input_audio_buffer.commit`. Session format is signed little-endian mono PCM16 at 24 kHz with null noise reduction and turn detection. The gateway preserves split samples across appends, continuously resamples to 16 kHz, flushes on commit, and fully resets uncommitted input on clear.

Standard server events cover session creation and updates, commit acknowledgment, item creation, transcription deltas, completion, failure, and errors. Clients may negotiate `item.input_audio_transcription.hypothesis`; the extension emits revisioned replacement snapshots containing the complete transcript and its finalized, agreed, and tentative regions. Completion is authoritative.

## Ownership and bounds

One interim worker and optional final worker are shared across clients. Workers retain no session, take, guidance, history, or transcript state between jobs. Each admitted job keeps an explicit generation work guard until cancellation is observed or native decode returns.

One session owns its uncommitted input and up to four independently finalizing committed items. `Take` is the only per-take abstraction and owns guidance, finalized history, segment aggregation, completion, and failure. Commit preserves the provisional item ID and durable lineage. Clear cancels only uncommitted work.

The fixed limits are:

- 8 active Realtime sessions
- 4 committed items per session
- 8 queued interim jobs and 8 queued final jobs
- 16 ordinary session results, plus reserved terminal and replaceable hypothesis slots
- 4 final segments per item
- 8 retained cancellation joins per session
- 15 MiB decoded audio per append
- 30 seconds of uncommitted audio
- 100 ms minimum committed audio

Capacity and capacity-plus-one tests pin each bound. Authoritative segments and terminal outcomes never use lossy admission.

## Profile replacement

Artifact preparation starts no worker. Replacement closes admission, installs a fresh rollback epoch, cancels old work, and waits for explicit request and job ownership to drain. Old workers then shut down and join before the new generation starts under one deadline.

The new generation remains unpublished until profile persistence succeeds. Persistence prepares and syncs a temporary file, atomically replaces the authoritative state, and syncs the parent where supported. Determinate failure reconstructs the old generation. Indeterminate persistence or non-preemptible startup timeout invalidates staged state and requests controlled process shutdown. Replacement never detaches a native worker or claims cancellation of a non-preemptible native call.

## Workshop path

The browser's `SpeechCaptureService` owns the microphone graph and emits little-endian mono PCM16 at 24 kHz. `RealtimeTranscriptionService` owns protocol negotiation and reconnect backoff. The view keeps one reversible editor range per take and replaces that range from hypothesis snapshots until completion.

Every browser Realtime socket receives an immutable, monotonically increasing generation. Decoded events, readiness and failure notifications, and append, commit, and clear results carry that generation. The take registry accepts newer readiness, treats duplicate readiness as idempotent, and ignores older readiness or any user, audio, capture, wire, error, or server input whose generation is not active.

Pending requests, client-event correlations, commit expectations, item bindings, and retired item tombstones are generation-scoped. Connection loss performs the existing editor and capture rollback, clears the closed generation's wire identity, and retains only local capture-stop ownership until it settles or a newer generation supersedes it. A new socket can therefore reuse an item ID immediately, while late callbacks and frames from the old socket cannot mutate the new take. The service also retains its current-socket callback guard.

The Workshop server exposes `/v1/realtime` on its own origin. It validates Origin, rejects subprotocols, attaches the Gateway credential upstream, preserves text, binary, close code, and close reason, and bounds relay writes. It does not parse speech JSON, report speech capability, or own speech status.

## Architecture and CI gates

- `node tools/check-stt-architecture.mjs` pins Cargo 1.89, `cargo-modules` 0.25.0, and `cargo-public-api` 0.52.0; rejects malformed tool output; proves every STT production-library module graph acyclic; and requires exact public-root counts.
- `cargo test -p gateway-stt --test it architecture` enforces exact final workspace edges, exact source manifests, the 500-line maximum for every source module, unsafe isolation, zero legacy speech seams, generic discovery, explicit lifecycle ownership, and transactional replacement.
- Normal CI installs only config UI dependencies before building Gateway, proving the default Gateway build cannot invoke Workshop UI tooling.
- Normal CI runs the architecture driver tests, the architecture driver, and the Rust architecture suite before formatting, linting, and tests.
- Miri runs backend-neutral worker, generation, queue, audio, registry, item, mailbox, and replacement-state targets. Native FFI, callbacks, sockets, and model loading remain on native CI.

## Debt result

The initial architecture-ratchet snapshot contained one temporary workspace edge, six migration-target exceptions, three source modules above 500 lines, a maximum module size of 712 lines, and a `gateway-stt` public-root allowance of 9.

The final snapshot contains zero temporary edges, zero migration exceptions, zero modules above 500 lines, a maximum module size of 481 lines, and exact public-root counts of 6, 7, 2, and 6. The forbidden `gateway-stt -> workshop-server` edge fell from one to zero. The larger final source total reflects the delivered Realtime protocol, ownership, and test surface; the debt measures are responsibility size, dependency direction, cycles, unsafe isolation, and public exposure, all enforced as failing gates.
