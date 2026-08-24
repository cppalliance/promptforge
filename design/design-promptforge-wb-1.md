# Design Choices: promptforge-wb stage 1

Running log of design choices for the PromptForge Workbench stage 1 build. Each entry states the choice, the evidence behind it, and the cost it carries.

## 1. Two crates, promptforge-wb-server and promptforge-wb, as workspace members

- Choice: the workbench is split into an HTTP server binary (`promptforge-wb-server`) and a desktop window shell binary (`promptforge-wb`), both globbed into the promptforge workspace as `crates/*` members.
- Evidence: the workspace already ships one binary per concern (gateway, mcp-server, cli), and the workbench needs a local HTTP API the shell and a browser can both reach, which is two processes with different dependency stacks.
- Cost: two binaries to build, run, and eventually package together; the shell cannot call server internals directly and must go through the HTTP API.

## 2. axum for the HTTP server

- Choice: the workbench server is built on axum 0.8 with tower for testing.
- Evidence: axum is the ecosystem default HTTP server in the rust rulebook, is already a workspace dependency used by promptforge-gateway, and its Router is directly testable with tower's `ServiceExt::oneshot` without binding a port.
- Cost: pulls the tokio/tower stack into the workbench; handlers are async even where the work is synchronous.

## 3. Default bind 127.0.0.1:7910

- Choice: the server binds to 127.0.0.1:7910 when no override is given.
- Evidence: the workbench is a local companion to the desktop shell, so loopback-only is the safe default; 7910 is an unassigned high port that does not collide with common development servers.
- Cost: the port is fixed rather than discovered, so a collision fails the bind instead of picking another port; remote access requires an explicit future override.

## 4. wry/tao deferred to the shell step

- Choice: `promptforge-wb` is an empty binary skeleton; the wry/tao window and its dependency tree are not added yet.
- Evidence: wry/tao pull in large platform GUI stacks (WebView2, gtk) that dominate build time, and nothing in the scaffolding step needs a window; keeping them out lets the workspace build fast while the server takes shape.
- Cost: the shell cannot be run end to end until the shell step lands, and the wry/tao integration risk is concentrated there instead of spread out.

## 5. Fresh config module in promptforge-wb-server, not promptforge-gateway-config reuse

- Choice: `workbench.toml` loading lives in a small `config` module inside promptforge-wb-server; the promptforge-gateway-config crate is not a dependency.
- Evidence: gateway-config's `Config` models the gateway's own server (models, endpoints, dominions, profiles, recursive includes), a different shape from the workbench's client-side view (`gateway.base_url`, `gateway.api_key`, `tape.path`, `server.bind`), and its `${VAR}` interpolation is `pub(crate)`, so reuse would require widening that crate's public surface for a consumer with no overlap in schema. The workbench mirrors its convention instead: parse TOML to a `toml::Value`, interpolate string leaves only (CFG-007 semantics), then deserialize, with `$$` as a literal `$` and unresolved variables as errors.
- Cost: about 80 lines of interpolation logic duplicated in spirit; the two crates now evolve the same convention in parallel and could drift.

## 6. Verbatim relay via raw bytes, statuses passed through

- Choice: the gateway client returns status plus raw body bytes, and the routes relay them with content-type `application/json`; a non-success gateway status is relayed, not mapped to a workbench error.
- Evidence: the step requires byte-for-byte relay, and re-serializing a `serde_json::Value` cannot guarantee key order (serde_json's default map sorts keys), so raw bytes are the only honest pass-through; relaying the gateway's own error envelopes keeps failure semantics in one place.
- Cost: the workbench cannot inspect or transform gateway responses at this layer, and the content-type is asserted rather than copied from the gateway response.

## 7. Route shapes: GET /v1/models mirrors the gateway, POST /chat is workbench-local

- Choice: the models proxy keeps the gateway's own path `/v1/models`; chat is `POST /chat` accepting `{"model", "messages"}` as a typed `ChatRequest` whose messages are unchecked `serde_json::Value` items; handlers reach the gateway client through axum `State<AppState>`.
- Evidence: mirroring `/v1/models` lets OpenAI-aware clients point at the workbench unchanged, while `/chat` is named for the workbench UI and typing only model plus messages keeps the stage-2 contract minimal; axum state is the idiomatic way to share one connection-pooled client across handlers.
- Cost: two naming conventions on one server, and extra fields in a chat request are silently dropped rather than forwarded.

## 8. Error design: one thiserror enum per module, boxed sources

- Choice: `ConfigError` (Read / Parse / UnresolvedVar / Interpolation) and `GatewayError` (Build / Transport / ReadBody), both `#[non_exhaustive]`, with dependency errors boxed as `Box<dyn Error + Send + Sync>` behind `#[source]`; handlers collapse any `GatewayError` to `502 Bad Gateway` with a JSON envelope.
- Evidence: matches the rust rulebook's one-enum-per-unit-of-fallibility and the gateway crate's own convention of boxing reqwest errors so dependency types stay out of the public API; a missing `workbench.toml` surfaces as `Read` naming the expected path.
- Cost: boxing erases the concrete source type for any caller that wanted to downcast, and the 502 mapping loses the distinction between connect, transport, and body-read failures at the HTTP boundary.

## 9. Tape event schema: six fields with full request and response JSON

- Choice: each tape line is one JSON object: `ts` (RFC 3339 UTC string), `kind` (`"chat"`), `model`, `request` (the request body exactly as received), `response` (the gateway body parsed to JSON, or a plain string if it was not JSON), `latency_ms` (u64, measured with `Instant` around the gateway call only). Timestamp rendering follows the promptforge-core `now_rfc3339_checked` convention: the `time` crate with a checked format error, never an empty string.
- Evidence: the step fixes the field list; storing full `Value`s keeps the tape replayable without the workbench understanding message shapes; recording the body as received (not the re-serialized `ChatRequest`) preserves extra client fields the relay drops; `Instant` around `chat_completion` keeps tape and relay overhead out of the number.
- Cost: prompts and completions land on disk verbatim, so the tape file is sensitive data; a non-JSON gateway body is stored as a string rather than dropped; transport failures produce no event, since there is no response to record.

## 10. Tape writer: std mutex around a boxed writer, not a channel

- Choice: `Tape` is `Arc`-shared state holding `std::sync::Mutex<Box<dyn Write + Send>>`; `record` is a sync method the chat handler calls through `tokio::task::spawn_blocking`; the file is opened append-plus-create with no `BufWriter`, one `write_all` per event.
- Evidence: the rust rulebook puts a short critical section with no `.await` on `std::sync::Mutex` and reserves an owner task for contended resources, and chat concurrency here is a handful of requests; `spawn_blocking` keeps disk stalls (antivirus, network drives) off the executor; the boxed `dyn Write` is what lets tests inject a failing writer; a `BufWriter` would need a flush per event anyway at one line per chat.
- Cost: a poisoned mutex is recovered with `into_inner`, so a panic mid-write could leave one partial line that later events do not repair; `spawn_blocking` adds a thread hop per event; under concurrent chats, event order is lock-acquisition order, not completion order.

## 11. Tape write failures are logged and swallowed; open failure is fatal

- Choice: `Tape::record` returns `Result<(), TapeError>`; the chat handler logs a failure with `tracing::error!` and still relays the gateway response, while `AppState::new` refuses to build if the tape cannot be opened; `main` initializes `tracing-subscriber` so the log line actually lands somewhere.
- Evidence: the step requires write errors to be visible yet never fail the user's chat - the `Result` makes the error visible to the caller (the handler) and the tracing log makes it visible to the operator, matching the gateway crate's own `%error` logging style; an unopenable tape at startup means every event would be lost, a configuration error worth failing fast over rather than serving a silently dead tape.
- Cost: a runtime write failure has no user-facing signal, only the log; if the disk fails mid-session the workbench serves untaped chats until restart.

## 12. SSE parsing: hand-rolled incremental decoder, no eventsource-stream

- Choice: the gateway client decodes SSE itself - a private `SseDecoder` (buffer partial bytes, split on `\n`, collect `data:` lines, dispatch on the blank line) layered over reqwest's `bytes_stream` via `stream::try_unfold`, exposed as `SsePayloadStream = Pin<Box<dyn Stream<Item = Result<String, GatewayError>> + Send>>` of verbatim `data:` payloads. The `eventsource-stream` crate was evaluated and not added; reqwest gained its `stream` feature and futures-util entered the crate from the existing workspace tree.
- Evidence: the rust rulebook adds a dependency only when the code exceeds roughly 100 lines and earns its tree; this decoder is about 60 lines, and an OpenAI-compatible stream needs only `data:` fields - no `event:` dispatch, no `id:` or Last-Event-ID reconnection, no `retry:` handling - so most of eventsource-stream's surface would be dead weight. The decoder is a pure synchronous core, so chunk-boundary, CRLF, multi-line `data:`, and EOF-flush behavior are unit-tested without a socket.
- Cost: the SSE spec's edge cases are ours to maintain; a gateway emitting exotic framing exercises code no third party has battle-tested, and the boxed `dyn Stream` return type costs `Clone` and a nameable type (accepted: callers only ever poll it).

## 13. Stream dispatch and the declined-stream relay

- Choice: `POST /chat` reads `"stream": true` from the raw request JSON before dispatching; streaming requests go to `GatewayClient::chat_completion_stream`, which serializes the typed `ChatRequest` and inserts `"stream": true`. A non-success gateway status on a streaming request is buffered and relayed verbatim with its status, and taped, exactly like a buffered chat.
- Evidence: the step fixes `"stream": true` as the trigger; reading it from the raw `Value` keeps the typed contract (model plus messages) unchanged; relaying error envelopes keeps failure semantics at the gateway, matching design entry 6; reusing the buffered tape path for declined streams keeps one taping convention.
- Cost: `"stream": false` or a non-boolean `stream` silently takes the buffered path; the streaming gateway call serializes a slightly different body than the buffered one (the extra field), and extra client fields are still dropped on both paths (design entry 7).

## 14. Assembled-response tape policy for streams

- Choice: a streamed chat tapes exactly one event after the terminal `[DONE]` (or a clean EOF), with `response` set to the concatenation of every event's `choices[0].delta.content` as a plain JSON string; role-priming and usage events contribute nothing. `latency_ms` spans the whole stream, from request to terminal event. The write runs inside the response body stream's own finalizer (an `unfold` state consumed exactly once), so the tape event cannot precede the last byte sent to the client.
- Evidence: the step fixes the one-event-with-assembled-response requirement; the tape schema already admits a plain-string `response` (design entry 9); folding the write into the body stream guarantees the exactly-once ordering without a background task racing the client.
- Cost: streamed tape events are shape-inconsistent with buffered ones (a string versus the gateway's JSON object), so tape readers must branch on shape; a client that disconnects before the stream ends leaves no tape event at all, because hyper drops the body stream without running the finalizer.

## 15. Mid-stream error taping

- Choice: a transport error mid-stream ends the workbench's SSE response after the last good event - no fabricated terminal frame - and tapes one event whose `response` is `{"error": <message>, "content": <partial assembly>}`.
- Evidence: the step requires an error note and never a silent gap; the 200 status and SSE headers are already committed when a mid-stream failure surfaces, so the only honest client signal is a truncated stream without `[DONE]`, while the tape carries the failure for the operator.
- Cost: the client must infer failure from the missing `[DONE]` rather than an explicit error frame, and the taped message is reqwest's transport wording, not a gateway error envelope.

## 16. UI assets embedded with include_str!, not served from a directory

- Choice: `index.html`, `app.js`, `style.css`, and the vendored `markdown-it.min.js` live in `crates/promptforge-wb-server/ui/` and are compiled into the binary with `include_str!`; routes `GET /`, `/app.js`, `/style.css`, and `/markdown-it.min.js` serve the embedded strings with explicit content types.
- Evidence: the workbench is a local companion app where single-binary deployment matters - the server can be launched from any working directory without locating its asset tree, and there is no runtime path resolution to fail; the UI is four small files, so the dev-iteration cost of a recompile per edit is seconds, and `include_str!` gives the compiler a rebuild dependency on the assets for free.
- Cost: editing the UI requires a recompile rather than a browser refresh against a live directory, and the binary grows by the asset size (about 130 KB, dominated by markdown-it); cache headers are not set, so the browser revalidates every load, which is fine on loopback.

## 17. markdown-it vendored into ui/, no CDN

- Choice: markdown-it 14.1.0 (minified, MIT) is checked into `crates/promptforge-wb-server/ui/markdown-it.min.js` and served as a static asset; assistant bubbles re-render the assembled markdown through it on every delta.
- Evidence: the step forbids CDN dependencies, and the workbench is a local tool that must work offline and must not depend on a third-party host's availability or integrity; markdown-it is the reference CommonMark renderer for the browser and rendering the full accumulated string per delta is cheap at chat-message sizes, so no incremental-parse machinery is needed.
- Cost: the vendored copy is a manual upgrade point - security or bug fixes land only when someone re-downloads the file; re-rendering the whole message per delta is O(n^2) in message length, acceptable for chat replies but a known ceiling.

## 18. fetch plus ReadableStream for the POST SSE stream, not EventSource

- Choice: the UI streams chat with `fetch("/chat", {method: "POST"})` and reads `response.body` through a `TextDecoderStream`, splitting on blank lines and joining `data:` lines by hand; EventSource is not used.
- Evidence: EventSource only issues GET requests and cannot carry the `{"model", "messages"}` JSON body, so the POST contract of `/chat` rules it out outright; the manual SSE framing parser is about fifteen lines because the stream carries only `data:` fields, mirroring the server's own decoder rationale (design entry 12).
- Cost: no automatic reconnection or Last-Event-ID, neither of which a chat round-trip wants anyway; the hand-rolled parser inherits the same spec edge-case maintenance burden as the server-side decoder.

## 19. Dark-only palette: near-black neutrals, one muted blue-violet accent

- Choice: the UI is dark-mode only - near-black backgrounds (`#0d0e12` base, `#14161c` raised), 1px `#262a33` borders, a single muted blue-violet accent (`#7c7fd4`), system font stack for prose, and a monospace stack for code. User messages are right-aligned in bordered bubbles; assistant messages render full-width markdown; a blinking accent cursor marks the streaming bubble.
- Evidence: the step fixes the Cursor-like layout and dark palette; a single accent keeps the chrome quiet so the prose dominates, and the system font stack avoids shipping font files while matching the host OS; no light theme halves the palette surface to maintain.
- Cost: users who prefer light mode have no option; the palette is hardcoded in CSS custom properties rather than derived from OS preferences, so it cannot follow a system accent color.

## 20. UI behavior verified by hand, server behavior by tests

- Choice: the static-asset routes are covered by server tests (200, content type, non-empty body), while the JavaScript behavior - Enter to send, live delta append, markdown re-render, model picker population, error bubbles - is verified manually in a browser against a running server.
- Evidence: the workspace has no browser automation stack (no headless Chromium, no JS test runner), and adding one for four behaviors would dwarf the code under test; the Rust-side contract the JS depends on (SSE framing, `[DONE]`, catalog shape) is already pinned by the server tests.
- Cost: a regression in `app.js` is caught only by a human clicking through the UI; the manual verification step has to be repeated whenever the SSE contract or the asset markup changes.

## 21. Voice transport: binary WebSocket messages of f32 PCM, not base64 text

- Choice: `GET /voice` upgrades to a WebSocket; PCM flows as binary messages, one per AudioWorklet block (typically 128 frames), each frame a little-endian f32 sample at 16 kHz mono; control messages and the server's reply are text.
- Evidence: the WebSocket protocol has a binary opcode for exactly this; base64 inside text frames would grow the payload by a third and add an encode/decode pair on both ends for no interop gain, since both peers are ours; on the browser side `ws.binaryType = "arraybuffer"` plus `ws.send(buffer)` moves the worklet's block without a copy into a string, and axum's `Message::Binary` delivers `Bytes` with no UTF-8 validation pass.
- Cost: the stream is opaque to text-based inspection (browser devtools render binary frames poorly), and the 4-byte little-endian framing is convention rather than self-describing - a mismatched client is detected only by the frame count being off.

## 22. Voice control protocol: "start" resets, "stop" reports, one JSON reply

- Choice: control messages are the bare words `start` and `stop`; `start` zeroes the per-session counter so one socket carries multiple takes; on `stop` the server replies with exactly one text message, `{"frames":N}`; unknown text is ignored; the socket closing ends the session. The server counts frames (4-byte samples), not messages.
- Evidence: the step fixes the start/stop verbs and the single frame-count reply; bare words are the smallest contract both sides can assert on, while JSON on the reply leaves room for the next step's transcription fields without changing the verbs; counting samples rather than messages makes the reply independent of the client's block size, which the test pins by sending two different-sized blocks (128 and 64 frames) and asserting 192.
- Cost: no message ids or versioning - a richer future protocol must version the verbs or move to envelopes everywhere; frames arriving before `start` are counted rather than rejected, and a trailing partial sample (a payload not divisible by four) is silently dropped from the count.

## 23. Push-to-talk UX: click-to-toggle mic button, transient status notes

- Choice: a mic-icon button sits left of the input box in the composer; clicking toggles capture (click again to stop), the button pulses red while recording, and outcomes land as transient notes in a status line under the composer - the frame-count reply on success, a visible error note on mic-permission denial or socket failure. Notes auto-dismiss after eight seconds.
- Evidence: the step asks for push-to-talk with a visible state change and a stop path; click-to-toggle works with mouse, touch, and keyboard (a real `<button>`, so Enter and Space toggle it) where hold-to-talk excludes keyboard users and is clumsy on trackpads; a transient note near the composer keeps capture bookkeeping out of the chat transcript, matching the error-bubble convention for chat failures while staying lighter weight.
- Cost: toggle is not true hold-to-talk - an accidental click starts capture and the mic stays live until clicked again; the auto-dismiss means a missed frame count is gone from the UI (the server's tracing log keeps it).

## 24. Capture pipeline: AudioWorkletNode with context-level resample to 16 kHz

- Choice: capture runs in an `AudioWorkletNode` fed by `getUserMedia({channelCount: 1, sampleRate: 16000})`; the `AudioContext` is constructed with `sampleRate: 16000` so the engine resamples before the worklet sees the data; each block is copied and posted to the page, which forwards it over the socket; the worklet is a served static asset (`/pcm-worklet.js`), embedded with `include_str!` like the other UI files.
- Evidence: ScriptProcessorNode is deprecated and runs on the main thread, where UI work can starve audio; an AudioWorklet runs on the audio rendering thread; constraining the context rate makes the browser's own resampler (maintained, SIMD-tuned code) guarantee the 16 kHz wire format regardless of the device's native rate, instead of shipping our own resampler; a served worklet file follows the embedded-asset convention (design entry 16) and picks up the same route test as the other assets.
- Cost: one more embedded asset and route; the worklet copies every block because the engine reuses its input buffers - an allocation per roughly 8 ms that `postMessage` makes unavoidable; `AudioContext({sampleRate})` is unsupported on very old Safari, which the UI surfaces as the generic capture-unavailable error.

## 25. Voice UI verified by hand, voice protocol by a live-socket test

- Choice: the `/voice` contract is pinned by server tests that bind the route on a loopback port and drive it with a real tokio-tungstenite client (start, two binary PCM blocks, stop, assert the `{"frames":N}` reply; plus a start-resets-take test and a route-mounting test), while the browser behavior - permission prompt and denial note, recording state, frame streaming, stop reply note - is verified manually in a browser against a running server, per the design entry 20 split. tokio-tungstenite is pinned to 0.29 so the resolver unifies it with the copy axum's `ws` feature already depends on.
- Evidence: tower's `oneshot` cannot drive a WebSocket upgrade (no hyper `OnUpgrade` extension exists without a real server), so a bound socket with a real client is the smallest honest harness; the workspace still has no browser automation stack, and the JS-side contract (binary frames out, one JSON text reply in) is exactly what the server tests pin.
- Cost: the test spins a real server per case (loopback, ephemeral port - milliseconds each); the manual browser pass must be repeated whenever the wire contract or the composer markup changes. Verified this step: server suite green (44 passed), `node --check` on both JS files, asset routes covered by tests; the in-browser mic pass is the human step.

## 26. Voice endpoint size and rate posture: axum defaults accepted, stated for the record

- Choice: `/voice` sets no limits of its own; axum's default `WebSocketConfig` caps a message at 64 MiB and a frame at 16 MiB, and those caps are the whole posture. The frame counter is a per-connection u64; there is no per-session duration cap, no rate limit, and no authentication on the route.
- Evidence: the server binds loopback by default (design entry 3) and the workbench is a single-user local tool, so the only client is the page the server itself served; the handler never buffers payloads - each binary message is measured by length and dropped - so memory in use is bounded by one capped message, and a hostile process already on the loopback interface has simpler ways to harm the machine than this endpoint.
- Cost: overriding `server.bind` to a non-loopback address exposes an unauthenticated endpoint that accepts arbitrary binary streams, and any such deployment must revisit this entry; a take of unbounded length accumulates only a counter, but the inbound stream itself is unthrottled below the 64 MiB message cap.

## 27. whisper-rs 0.16 for on-device interim transcription

- Choice: transcription runs through `whisper-rs = "0.16"` (current on crates.io as of this step; 0.15.1 was the prior release), the safe Rust bindings over whisper.cpp. Models are GGML/GGUF files loaded from paths named in `workbench.toml`.
- Evidence: whisper.cpp is the reference local speech-to-text runtime, and whisper-rs is its maintained safe binding - the crate's own `unsafe` stays behind its API, so the workspace's `unsafe_code = "forbid"` holds for our code. The build needs cmake plus libclang on the machine (whisper-rs-sys compiles whisper.cpp and runs bindgen at build time); both were installed out of band and `LIBCLANG_PATH` is set to the LLVM install's `bin` directory.
- Cost: a C++ toolchain dependency (cmake, MSVC, libclang) on every machine that builds the server, a few minutes of first-build time, and a ~75 MiB model file per voice profile; whisper-rs's release cadence trails whisper.cpp, so upstream fixes arrive on the binding's schedule.

## 28. Voice config: `[voice]` with per-role model paths, 5 s window, 800 ms cadence

- Choice: `workbench.toml` gains an optional `[voice]` section: `interim_model` (path to the streaming model), `final_model` (path, parsed now and loaded by the next step's final pass), `window_seconds` (default 5), and `interval_ms` (default 800). An empty `interim_model` disables transcription entirely; the endpoint then keeps its capture-and-count behavior and replies with empty transcripts.
- Evidence: 5 s is the window whisper.cpp's own streaming examples converge on - long enough for the tiny model to form a sentence, short enough that a pass finishes well under the interval on a desktop CPU; 800 ms gives the user visible rewrites roughly once a second without queueing passes back-to-back (a tiny-model pass over 5 s of audio takes a few hundred milliseconds on 4 threads). Two model paths let the cheap model serve the hot loop while a bigger one serves the once-per-take final pass.
- Cost: two more tunables with no auto-tuning, and a server with `[voice]` configured fails startup when the model path is bad (deliberate, matching the tape's fail-fast posture in design entry 11); `final_model` is dead config until the next step lands.

## 29. Transcription worker: one dedicated thread, jobs over a channel

- Choice: the whisper context and state live on a single dedicated `std::thread` per server (built once at startup inside `AppState::new`); callers send owned sample buffers through a `std::sync::mpsc` channel and await the transcript on a `tokio::sync::oneshot`. Inference parameters are fixed for the streaming case: greedy decoding, `no_context` (each window stands alone), `single_segment`, timestamps and all console printing off, `suppress_blank` and `suppress_nst` on.
- Evidence: whisper inference is blocking CPU work, and the rust rulebook keeps that off the executor - a long-lived worker plus a channel is its prescribed shape, and it sidesteps `Send`/`Sync` questions by never moving the context after construction. One worker serializes passes, which is correct for a single-user tool: concurrent sessions would otherwise contend on the same cores. `no_context` matters most: with it off, the decoder primes on the previous pass's output and a hallucination compounds across the sliding window. Loading at startup makes a bad model path a startup error and keeps the first mic click fast.
- Cost: one model's worth of RAM is held for the process lifetime even when voice goes unused; a slow pass delays later ones (no preemption); startup blocks on the model load (about a second for tiny).

## 30. Interim protocol: `{"type":"interim"}`, then `{"type":"final","text","frames"}`

- Choice: while a take records, the server pushes `{"type":"interim","text":"..."}` text messages, each a full rewrite of the trailing window; on `stop` it sends one `{"type":"final","text":"...","frames":N}` where the text is one last pass over the trailing window (the real final-model pass is the next step). The take's PCM is buffered for the whole session in a `Vec<f32>` behind a `std::sync::Mutex`; the interim loop and the receive loop share it, and outbound messages from both loops funnel through one `tokio::sync::mpsc` channel into a writer task.
- Evidence: a full rewrite per interim is the honest output of a sliding-window decoder - there is no stable prefix to append to - and the client just replaces its text. Keeping `frames` in the final message preserves the old reply's diagnostic value. The single outbound channel keeps the two producer tasks from interleaving bytes on the socket; the mutex is the rulebook's short-critical-section case, since no `.await` is ever held across it.
- Cost: the final text this step is only the last-window transcript, so a take longer than the window loses its beginning until the real final pass lands; the buffered take grows about 3.8 MB per minute of audio, unbounded (a cap belongs with the final-pass step); an in-flight interim pass at `stop` is discarded.

## 31. Silence suppression: RMS gate first, whitespace filter second

- Choice: two layers. Before any whisper call, a window whose RMS amplitude is under 0.001 (-60 dBFS) is skipped entirely, and interim windows shorter than half a second are skipped too; after a pass, empty or whitespace-only transcripts are never sent. The same gate decides the final message's text: silence yields `""`.
- Evidence: whisper hallucinating plausible phrases ("Thank you.", "Thanks for watching.") on silent or near-silent input is a documented failure mode of the model family - it was trained to always say something. Not calling the model on silence is cheaper and more reliable than filtering its output, and 0.001 RMS sits far above the noise floor of a browser stream that already ran echo cancellation and noise suppression, yet far below speech (0.02+). The post-pass trim catches the residual case of a quiet-but-not-silent window decoding to nothing.
- Cost: speech quieter than -60 dBFS RMS is dropped unheard (a gain problem, not a transcription problem); the fixed threshold cannot adapt to an unusually noisy room, where a loud fan passes the gate and the model may still hallucinate - accepted until real usage says otherwise.

## 32. Test model and speech fixture, downloaded out of band and gitignored

- Choice: tests use `ggml-tiny.en.bin` (whisper tiny, English-only, ~74 MiB) from `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin`, and the speech fixture is whisper.cpp's own `samples/jfk.wav` (16 kHz mono s16, 11 s, "ask not what your country can do for you") from `https://github.com/ggerganov/whisper.cpp/raw/master/samples/jfk.wav`. Both live in `crates/promptforge-wb-server/tests/fixtures/`, which is gitignored; a missing model fails the test with the download URL in the message. The expected word is "country", spoken twice in the clip.
- Evidence: tiny is the smallest model that transcribes the jfk clip reliably, and the English-only variant is smaller and faster than the multilingual one for the same accuracy on English speech; the jfk clip is the canonical whisper.cpp smoke-test input - real speech with a known transcript, already 16 kHz mono, no synthesis required, and a presidential address (public domain). Gitignoring 74 MiB of binaries keeps the repo lean; the URLs above are the re-fetch instructions.
- Cost: a fresh clone cannot run the three transcription tests until someone downloads the fixtures (the failure message says how); tiny is noticeably weaker than the models a user would configure, so the tests prove the pipeline, not the transcription quality; hound joins the dev-dependencies to parse the WAV.

## 33. Interim UI: live rewrite area above the composer, final lands in the input box

- Choice: a dedicated interim line sits between the message list and the composer; each `interim` message replaces its contents (the user watches the take rewrite), styled dim and italic to read as provisional. On `final`, the area clears and the transcript becomes the input box's contents - focused, ready to edit and send. An empty final leaves the input alone and posts a "no speech detected" note with the frame count.
- Evidence: the step fixes the replace-in-place interim and the final-to-input handoff; putting the interim directly above the composer keeps it adjacent to where the text will land, and reusing the voice-status note for the empty case keeps the chrome to two small areas. The input box (not auto-send) is the safe terminus: transcription is lossy, and the user edits before anything reaches the model.
- Cost: one more always-present DOM element; the final transcript replaces any text the user had typed in the input box mid-take (accepted: push-to-talk while typing is a corner case, and merging heuristics would guess wrong more often); as with the rest of the UI (design entry 20), behavior is verified in the browser by hand while the wire contract is pinned by the server tests.

## 34. Segmentation VAD: energy-based RMS-over-window, not whisper.cpp's vad.cpp

- Choice: segment boundaries are detected by a small pure-Rust `Segmenter` (src/segment.rs) that scans the take buffer in 30 ms frames, gates each frame on the same RMS threshold as the interim silence gate (0.001, design entry 31), and closes a segment when 700 ms of silence follows at least 250 ms of speech. whisper.cpp's `vad.cpp` (Silero-based, exposed by whisper-rs's `whisper_vad` module) was evaluated and not used.
- Evidence: the segmenter only needs to find sentence boundaries in an already noise-suppressed browser stream, and the RMS gate is the exact detector the interim path already trusts to separate speech from silence, so both paths agree on what silence is; vad.cpp would add a second model load (Silero weights), per-frame inference cost on the receive path, and a parameter set (thresholds, min speech/silence durations, speech padding) to tune, for boundary accuracy a transcript-assembly pipeline does not need - a slightly early or late cut costs nothing because conditioning (entry 35) carries context across the cut. The segmenter is a pure incremental state machine over a growing buffer, so it is unit-tested on synthetic tone-and-silence buffers with no model and no socket.
- Cost: a fixed 700 ms closing pause can split a slow, hesitant speaker's sentence across two segments (conditioning mitigates: the second half is decoded with the first half as its prompt), and a loud sustained noise (fan, keyboard) reads as speech and delays segment closure until the stop tail - accepted until real usage says otherwise, same posture as entry 31.

## 35. Final-pass conditioning: initial_prompt carries the accumulated transcript

- Choice: each final-pass segment is transcribed with `no_context` still on (decoder state never carries between passes) and the take's accumulated transcript passed explicitly through whisper's `set_initial_prompt`, sanitized (null bytes stripped, since the setter panics on them) and capped to the last 800 chars so the prompt fits whisper's 224-token budget with the recent context - the part that matters for continuity - kept.
- Evidence: `no_context` off would prime the decoder on the previous pass's raw decoder state, which is exactly the hallucination-compounding failure design entry 29 rejected for the interim loop; `initial_prompt` is whisper.cpp's supported mechanism for conditioning a pass on known-good prior text, and it is how domain vocabulary (names, jargon from earlier in the take) survives segmentation. The cap mirrors whisper.cpp's own behavior of truncating over-long prompts from the front. The accumulated transcript is built by joining segment transcripts with single spaces.
- Cost: the prompt is text, not decoder state, so conditioning costs a re-tokenization per segment and cannot carry acoustic context (speaker tone, emphasis); an early mis-transcription is baked into the conditioning of every later segment in the take - the same compounding risk as `no_context`, bounded by the final model being far more accurate than the interim one.

## 36. Final-pass worker: second whisper thread, worker-side transcript accumulation

- Choice: the final model (large-v3 in production config, the tiny fixture in tests) lives on its own dedicated thread (`whisper-final`) with its own context and state, fed by a FIFO `std::sync::mpsc` channel of `Reset` and `Segment` jobs. The accumulated transcript lives on the worker (`FinalPass`), not in the session task: each segment job is conditioned on whatever the worker has accumulated, and a segment's reply carries the full assembled transcript. `start` sends `Reset`; completed segments are fire-and-forget `submit`s; `stop` sends the tail and awaits its reply.
- Evidence: because the channel is FIFO and the worker accumulates, segment N is always conditioned on segments 1..N-1's finished transcripts - session-side accumulation cannot promise that, since segment N-1's transcript does not exist yet when segment N is detected and enqueued. Awaiting the tail's reply doubles as a drain: every earlier segment is complete by the time it arrives, so the stop path needs no separate synchronization. One worker serializes final passes, correct for a single-user tool, and mirrors the interim worker's shape (design entry 29). Conditioning is observable: `FinalPass` records the prompt used on the most recent segment, which the tests assert against.
- Cost: a second model's worth of RAM is held for the process lifetime (large-v3 is about 3 GB - the real reason `final_model` is a separate, optional path), and a bad `final_model` path fails startup like a bad interim path (entry 28's fail-fast posture); a slow segment delays the stop reply by however far the worker is behind.

## 37. Tail handling on stop: the unclosed remainder is the last segment job

- Choice: on `stop`, everything past the last closed segment boundary (`Segmenter::consumed()`) - in-progress speech, trailing silence, or nothing - is sent as one final `Segment` job and its reply awaited as the take's transcript. The worker applies the same gates as the interim path (half-second minimum, RMS silence check) and skips transcription of a silent or tiny tail, replying with the accumulated transcript unchanged.
- Evidence: treating the tail as an ordinary segment keeps one code path and one set of gates, and the FIFO channel makes the await a drain (entry 36), so the reply is guaranteed to include every background segment; reusing the silence gate keeps whisper from hallucinating a closing sentence over the take's trailing quiet, matching design entry 31. A take with no detected segments at all degrades gracefully: the whole take is the tail, transcribed once on stop - exactly the old behavior with the better model.
- Cost: the tail is transcribed as one possibly long pass (whisper.cpp windows it internally in 30 s chunks), so a user who talks for minutes without a 700 ms pause waits for that whole pass at stop - the pipelining only pays off when the VAD finds boundaries, which conversational speech provides.

## 38. Final-pass fallback policy: no final model, or a failed pass, uses the interim model

- Choice: when `final_model` is not configured, `stop` falls back to the previous behavior - one last interim-model pass over the trailing window - and logs an info line saying so; when the final pass itself errors mid-take, the failure is logged and the same interim fallback produces the reply. The client protocol is unchanged either way.
- Evidence: the step fixes the fallback for the unconfigured case; extending it to the error case keeps a broken large model (OOM, corrupt file discovered late) from turning every take into an empty transcript when a working interim model is sitting right there, matching the endpoint's existing never-fail-the-reply posture (design entry 11's logging convention). The info log makes the degraded mode visible to the operator without spamming - once per stop, not per segment.
- Cost: the fallback transcript covers only the trailing window, so a long take served by the fallback silently loses its beginning - the log line is the only signal; and a final-pass failure is retried nowhere, so a transient error costs that take's final quality.

## 39. wry 0.56 and tao 0.36 for the shell window

- Choice: the desktop shell uses wry 0.56 (0.56.1, current on crates.io as of this step) for the webview and tao 0.36 for the window and event loop, not the newer tao 0.37.
- Evidence: wry 0.56.1's own dev-dependency constraint is `tao ^0.36`, so 0.36 is the pairing wry is tested against; tao 0.37 shipped after this wry release and nothing in the step needs it. wry is the ecosystem default for embedding a system webview (WebView2 on Windows, which needs no extra SDK to build), and wry's own docs recommend tao as the windowing companion. Their `unsafe` stays behind their APIs, so the workspace's `unsafe_code = "forbid"` holds for our code.
- Cost: the wry/tao tree (windows, webview2-com) is the largest dependency stack in the workspace and dominates a cold build; both crates track Tauri's release cadence, so upgrades arrive on Tauri's schedule and should be taken as a pair.

## 40. In-process server on a dedicated thread, not a child process

- Choice: `promptforge-wb` starts the server by calling the new library entry point `promptforge_wb_server::spawn(config)`, which builds the state, binds the listener, and serves on a dedicated `std::thread` with its own multi-threaded tokio runtime. `spawn` blocks until the listener is bound - the bind is the readiness signal - and returns a `ServerHandle` carrying the bound base URL, a graceful-shutdown oneshot, and the thread handle. The shell then polls `GET /health` until it answers 200 before opening the window. Spawning the server binary as a child process was the alternative and was rejected.
- Evidence: in-process keeps one binary to ship and one process to supervise, and the bound address comes back as a value rather than being scraped from a child's stdout; the rulebook's one-runtime-per-process rule is satisfied by giving the server thread its own runtime instead of sharing one with the GUI thread, which must stay free for tao's event loop (tao requires the main thread). The health poll on top of the bind signal proves end-to-end serving, not just a bound socket, and it is what the step asked for. Graceful shutdown rides axum's `with_graceful_shutdown`, so window close drains the server instead of killing it mid-request.
- Cost: a server panic takes the whole process down with the window (a child process would isolate it), and the shell links the server's full dependency tree (whisper included) instead of leaving it in the server binary.

## 41. Config search order: beside the executable, then the current directory, then the profile

- Choice: the shell looks for `workbench.toml` beside the executable first, then in the current directory, then in `%USERPROFILE%\.promptforge\workbench.toml`; first found wins, and a total miss is an error naming every path tried in order.
- Evidence: beside-the-executable first makes a packaged layout (config shipped next to the binary) win over anything ambient, which is the least surprising behavior for an installed app; the current directory second preserves the server binary's existing launch-from-a-project-directory convention; the profile directory last is the per-user fallback. Naming every tried path in the error turns a silent wrong-config pickup into a one-glance diagnosis. The search is factored as a pure `candidates_from(exe_dir, cwd, home)` plus `first_existing`, so the order is pinned by tempdir tests rather than by reading the code.
- Cost: three locations means three places a stale config can shadow the intended one - mitigated only by the error message, which the happy path never prints; the server binary keeps its single-path behavior (`workbench.toml` in the current directory), so the two binaries resolve config differently.

## 42. No native menu; the close button is the quit gesture

- Choice: the window has no menu bar. The step allowed "File menu with Quit or just close behavior"; close behavior was chosen. Shutdown runs after tao's `run_return` returns, so both the close request and any future quit path funnel through the same server-shutdown code.
- Evidence: tao 0.36 moved menu support out of the crate (menus now live in Tauri's `muda` crate, with its own event channel separate from tao's `Event`), so a one-item `File > Quit` menu would cost a new dependency, a second event source to drain, and platform-specific attach code (`init_for_hwnd` on Windows) - all to duplicate what the window's close button already does. `run_return` (tao keeps the winit 0.28-style API) was chosen over the diverging `run` precisely so control returns to `main` and `ServerHandle::shutdown` runs unconditionally, including when window creation fails.
- Cost: there is no keyboard-accessible quit shortcut (Alt+F4 still works on Windows, as the OS-level close), and adding a real menu later means taking on `muda` and reworking the event loop; `run_return` is a platform extension trait, so a future tao upgrade that drops it forces a restructure.

## 43. Browser fallback: `server.open_browser` opens the system browser from the server binary

- Choice: `workbench.toml` gains an optional `server.open_browser` (default false). The server binary reads it after `spawn` returns and, when true, opens the bound URL with the `open` crate (5.4.2); a failure to open is logged with `tracing::warn` and serving continues. The shell ignores the flag - it has a window.
- Evidence: the step fixes the semantics (when true and no window is available, the server binary opens the browser - the browser-tab frame); the server binary is by definition the windowless frame, so the flag lives in its main. `open` is the ecosystem-default crate for launching a URL in the system browser; the std alternative (`cmd /c start` on Windows, `open` on macOS, `xdg-open` on Linux) is shell-quoting-sensitive and would be our code to maintain. Waiting until after `spawn`'s readiness signal means the browser never opens on a dead address. Log-and-continue matches the tape's failure posture (design entry 11): a browser that will not open is an environment problem, not a reason to kill a serving server whose address is in the log.
- Cost: one more dependency (small, no transitive tree of note); `open` spawns the platform handler and reports only launch failure, so a browser that opens the wrong profile or blocks localhost is invisible to us.

## 44. The window is verified by hand; discovery and readiness by tests

- Choice: automated tests cover the config search order (tempdir fixtures for each precedence pair and the names-every-path error), the health probe (a stub `TcpListener` answering 200, a 503-then-timeout case, a dead port, and a non-http URL), and the server entry point's readiness signal (spawn on an ephemeral port, assert `/health` answers 200 over a real request, assert shutdown releases the port, assert state and bind failures surface as `SpawnError` variants). The window itself - title, webview load, close-driven shutdown - is verified by hand, and no test opens a window.
- Evidence: tao requires the main thread and a real display, so a windowed test cannot run in CI or under `cargo test --workspace`; the design entry 20 split (server contract by tests, UI behavior by hand) already governs this crate. Everything up to the window boundary is pinned: the readiness test proves the exact signal the shell waits on, and the probe tests prove the shell's half of the handshake.
- Cost: a regression in `window.rs` (a dozen lines of event-loop wiring) is caught only by a human launching the shell; the health probe is a hand-rolled HTTP/1.0 GET, adequate for a loopback status line but not a general client.

## 45. Documentation: per-crate READMEs, example config at the repo root

- Choice: each workbench crate gets a README matching the existing crate convention (pitch, quick start, MSRV, BSL-1.0 license); the server's adds a full `workbench.toml` field table, the route table, and the out-of-band whisper model download note, while the shell's covers the three-path config search order and the `server.open_browser` browser fallback. The commented example config lives at the repository root as `workbench.example.toml`, not inside the crate. The workspace README's crate table lists both crates with `not published` in the crates.io column, since both are `publish = false`.
- Evidence: the root is where the workspace README's quick start already points users for `gateway.toml`, so the root is the more discoverable home for the example; the crates.io badge column has no honest badge for an unpublished crate, and `not published` says so plainly rather than linking a badge that 404s; keeping the field table in the server README (the crate that parses the config) puts the reference next to the code it describes, with the shell README linking to it instead of duplicating it.
- Cost: the field table is prose, not compiled - a new config field can ship without a README update, and nothing but review catches the drift.

## 46. Optional workbench.toml - environment-variable-only config path

- Choice: `workbench.toml` is now optional. When no TOML is found (by discovery or in the current directory), `Config::from_env()` builds a valid `Config` entirely from environment variables with sensible defaults. The only required variable is `PROMPTFORGE_GATEWAY_API_KEY`; all others default to the same values the TOML path uses. `discover_config()` returns `Option<PathBuf>` instead of erroring on a miss.
- Evidence: container and CI deployments pass secrets through environment variables, not config files; requiring a TOML file forces either a volume mount or a build-time bake step that exposes secrets in layers. The env-var names follow the `PROMPTFORGE_` prefix convention already used for `${PROMPTFORGE_GATEWAY_API_KEY}` interpolation inside the TOML. Existing TOML behavior is unchanged - the TOML remains the primary source when present.
- Cost: two config paths to maintain and test; a user who sets both a TOML and env vars might be surprised that the TOML wins unconditionally; error messages differ between the two paths (parse errors vs missing-var errors).

## 47. First-run config generation replaces the env-only path (supersedes 46)

- Choice: `Config::from_env` and its lookup-based test seam are deleted, one step after they shipped. When the shell's three-path search finds no `workbench.toml`, it creates `~/.promptforge/` if needed, writes a built-in default template there, logs the path, and loads it; the app never exits on missing config. `ConfigError` gains a typed `NotFound { path }` variant, split out of `Read` by `ErrorKind::NotFound`, so a caller can tell "no file" apart from "unreadable file" - and the `MissingEnvVar` / `InvalidEnvVar` variants die with the env-only path.
- Evidence: entry 46's env-only path existed to make config optional, but it left a first-run user staring at a missing-variable error and gave the generated-config plan two config paths to keep honest; generating a real file is strictly more discoverable - the user gets a commented artifact to edit, and the TOML path becomes the only path. The template's gateway fields are `${PROMPTFORGE_GATEWAY_URL}` / `${PROMPTFORGE_GATEWAY_API_KEY}` interpolations, so a machine with the variables set is configured from the first launch, keeping entry 46's container/CI benefit without a second code path. The server binary is unchanged: it still reads `workbench.toml` from the current directory only.
- Cost: the generated file is a second copy of the defaults to keep in sync with the schema (the template is pinned by a test that loads it, which catches drift that breaks parsing but not drift in comments); a user who deletes the profile file to "reset" gets a fresh template that silently discards nothing but their own edits.

## 48. Voice model source URLs are config fields (Amendment A)

- Choice: `VoiceConfig` gains `interim_source` and `final_source` string fields (serde-defaulted empty), and the generated template fills them with the whisper.cpp Hugging Face URLs for `ggml-large-v3-turbo.bin` and `ggml-large-v3.bin`. The plan's download-URL comments in the template are dropped - the real fields carry the same information in machine-readable form.
- Evidence: the gateway cache API step needs the download URLs somewhere a program can read them; comments are invisible to the loader, so the URLs had to become data. String fields with an empty default match the config's existing convention for optional values (the model paths themselves are pathbufs with an empty-means-disabled convention).
- Cost: two fields that nothing reads yet - they are inert until the cache integration lands, and a schema reader sees download hints the current build cannot act on.

## 49. Unset ${VAR} interpolates to the empty string (Amendment B)

- Choice: in `interpolate`, an unset environment variable now expands to `""` instead of failing with `ConfigError::UnresolvedVar`. A variable whose value is not valid Unicode still fails, and keeps the `UnresolvedVar` variant (its message now says so). No `${VAR:-default}` syntax exists, so nothing else had to be preserved.
- Evidence: the generated template references `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY`, which a first-run user has not set; erroring on unset would make the generated config fail to load on exactly the machines it was written for. Empty interpolation plus the entry-50 fallback turns "unset" into "the built-in defaults", which is the behavior the first-run feature needs. The non-Unicode case stays an error because interpolating lossy bytes into a TOML string would hide a real environment defect.
- Cost: a typo'd variable name now silently yields an empty value instead of a startup error naming the variable - the failure surfaces later and less clearly (an empty `api_key` means the gateway's 401, an empty path means the field's empty-default behavior); the workspace's other config loader (gateway-config) still errors on unset, so the two crates' interpolation semantics now diverge on this point.

## 50. Empty gateway.base_url falls back to the loopback default

- Choice: after interpolation and deserialization, an empty `gateway.base_url` is rewritten to `http://127.0.0.1:8081` (the new `DEFAULT_GATEWAY_BASE_URL` constant). An empty `gateway.api_key` is left empty, and `GatewayClient` now skips the `Authorization` header entirely when the key is empty - previously `.bearer_auth("")` sent a `Bearer ` header with no token.
- Evidence: with entry 49, the generated template's `${PROMPTFORGE_GATEWAY_URL}` resolves to empty on a machine without the variable, and an empty base URL would produce request URLs like `/v1/models` that fail at send time with reqwest's relative-URL error - a worse first-run failure than pointing at the loopback gateway default every other part of the workspace assumes. Sending no header for an empty key is what a gateway with auth disabled expects; a `Bearer ` header with an empty token is rejected by strict parsers and leaks the fact that auth was attempted.
- Cost: the fallback is invisible in the loaded `Config` (the field holds the default, not a marker that it was defaulted), so a user who explicitly sets `base_url = ""` cannot tell the difference from the file; and a gateway that genuinely requires an empty-token `Bearer` header - pathological, but legal - can no longer be expressed.

## 51. The generated template ships with voice disabled

- Choice: the generated template comments out `interim_model` and `final_model` (with a note to uncomment after downloading) instead of setting them to `~/.promptforge/models/...` as the plan's template showed. The `*_source` URLs (entry 48) remain as real fields.
- Evidence: the plan's template pointed the model paths at files that do not exist on first run, and `AppState::new` deliberately fails startup on a bad model path (entry 28) - so the generated config would have made the app exit on first run, contradicting this step's own "the app never exits on missing config" requirement. Nothing expands the `~` either, so the paths were doubly broken. With the paths commented out, `voice.enabled()` is false, startup skips model loading, and the cache-integration step downloads the models and loads them from the returned paths without depending on these config paths.
- Cost: voice is off until the user (or the cache integration) acts - the template cannot both enable voice and guarantee first-run startup; the commented-out paths duplicate the `*_source` URLs' filenames in comment form, one more place to drift.

## 52. Cache API reuses the reqwest blocking downloader, not hf-hub

- Choice: the gateway cache API is built on the existing reqwest-blocking download path in `local/artifacts/` (`download_with_progress`: streaming SHA-256, 256 GiB cap, scoped HF bearer token, connect/request timeouts), refactored into a reusable `local/cache.rs` `BlobCache`. The plan text said the downloader was the `hf-hub` crate; that was wrong - `hf-hub` is in the workspace dependency table but the gateway's local provisioning never used it - and the user confirmed reusing the reqwest code.
- Evidence: `artifacts/download.rs` already streams to disk with an incremental hasher and reports through the `DownloadProgress` trait, which is exactly the seam the SSE progress channel needs; adopting `hf-hub` now would replace a hardened, tested path (ART-003/004/006/007 confinement, locks, staging) with a second downloader to audit. The refactor widened selected `pub(super)` items to `pub(crate)`, extracted `download_client` and `lock_artifact` as free functions, and left the provisioning path (`ArtifactStore`) behavior unchanged.
- Cost: two cache writers (`ArtifactStore` provisioning, `BlobCache` on-demand) share the layout and the lock scheme, so both must be considered when the layout changes; the blocking client means every download occupies a `spawn_blocking` thread (entry 56).

## 53. Cache layout: shared models/<source-key>/<filename> slots with .meta.json sidecars

- Choice: cache-API blobs land in `<cache_root>/models/<source_cache_key>/<filename>` - the exact slot `ArtifactStore::ensure_model` computes for the same URL - and each completed download writes a `<filename>.meta.json` sidecar holding `{source, sha256, size_bytes}`. The sidecar name appends to the full filename (`model.gguf.meta.json`), so it cannot collide with the HF-metadata `.md` sidecar (`sidecar.rs`, which replaces the extension).
- Evidence: sharing the slot means a whisper model fetched through `POST /v1/cache` is a provisioning cache hit if an operator later configures the same URL as a `[[local_model]]` source (and the reverse), so multi-gigabyte weights are never stored twice; keying by `source_cache_key` keeps ART-004's guarantee that distinct URLs sharing a filename never collide. The sidecar carries the metadata the listing route needs, because re-deriving it means reading the blob.
- Cost: a blob downloaded by provisioning alone has no sidecar and is invisible to the cache API (entry 54's hit rule) - a `POST /v1/cache` for an already-provisioned URL re-downloads it once to earn the sidecar; the sidecar is trusted as the record of truth, so a blob swapped underneath its sidecar is reported with stale metadata until a pin mismatch forces a re-download.

## 54. Cache hit requires blob + sidecar + matching pin; publication is staged (Amendment E)

- Choice: a cache hit requires the blob and its sidecar both present, plus the sidecar's digest matching the request's `sha256` when one is named. Downloads stage to `<file>.part` and rename into place only after verification; any failure removes the `.part` and writes no sidecar. A pre-existing sidecar-less blob at the destination is replaced only after the new content verifies (Windows `rename` refuses an existing target, so the old blob is removed between verification and rename). Concurrent publishers of one source serialize on the existing artifact lock, with the hit test repeated under the lock so exactly one downloads.
- Evidence: the sidecar in the hit rule is what lets the check answer without hashing (Amendment C, entry 55); staging-then-rename mirrors `ensure_blob`'s ART-007 discipline so a crashed or failed download never leaves a partial blob at a final path; re-checking under the lock makes two simultaneous `POST /v1/cache` calls for one URL converge on one download, matching the racing-publishers guarantee provisioning already has.
- Cost: a sidecar-less blob is re-downloaded even when its bytes are identical (the adoption path - hash once, write the sidecar - was considered and deferred as extra surface for a one-time cost); the window between removing the old blob and renaming the replacement is small but nonzero, and a crash there loses the old blob (it is re-downloadable, so the loss is bounded).

## 55. GET /v1/cache reads sidecars, never re-hashes (Amendment C)

- Choice: the listing route walks `<cache_root>/models/<key>/` for `*.meta.json` sidecars and answers from their contents (`source`, `sha256`, `size_bytes`); blob bytes are never read. A sidecar whose blob is missing is skipped, a corrupt sidecar is logged and skipped, and a blob with no sidecar (a pre-existing local model file) is simply not a cache entry. The active profile's `[local].cache_dir` now rides on `LocalRuntime` (recorded at `start` even when no local models are configured, and swapped atomically on profile switch with the runtime itself), and each cache request resolves the root and opens a `BlobCache` inside `spawn_blocking`.
- Evidence: re-hashing a multi-gigabyte GGUF per GET would make the listing route unusable for exactly the models it exists for; the sidecar was written at download completion from the streaming hasher, so it is authoritative. Skipping (not erroring on) stale or corrupt sidecars keeps one bad file from 500ing the whole listing; the walk uses `file_type` without following links, so a planted symlinked slot is skipped rather than read through. Per-request `BlobCache` construction re-runs the ART-006 private-root enforcement on each call (two `icacls` spawns on Windows) - measurable but irrelevant next to the network operations these routes exist to drive, and it keeps gateway boot behavior unchanged when no cache is configured.
- Cost: listing metadata can go stale if an operator hand-edits the tree (accepted: the tree is owner-private, and a pin mismatch on the next POST forces a re-download); per-request root enforcement is redundant work when the local runtime already enforced it at boot.

## 56. POST /v1/cache: blocking download in spawn_blocking, progress over a bounded channel (Amendment D)

- Choice: the download runs inside `tokio::task::spawn_blocking` (the reqwest-blocking client must never touch the executor), and progress crosses to the async side through a bounded tokio mpsc channel (capacity 64) driven by a `ChannelProgress` implementing the existing `DownloadProgress` trait with `try_send`. Intermediate `downloading` samples drop under backpressure; the terminal `ready`/`error` event is not a sample - it is produced by awaiting the download task's `JoinHandle` after the channel closes, so the outcome can never be lost to a slow client. A cache hit is answered before any streaming with a plain JSON `{"path", "status": "ready"}`; validation (http/https URL with host and filename, 64-hex pin normalized by the existing `parse_expected_digest` boundary) happens before the store is opened, so a bad request is a 400 and never reaches the network.
- Evidence: `DownloadProgress` is the seam `artifacts/download.rs` already reports through, so the channel adapter is the minimal crossing; `blocking_send` was rejected because it panics on runtime threads and unbounded queues let a stalled client accumulate a sample per 64 KiB read for gigabytes; dropping intermediate samples is safe because each carries cumulative bytes, so any survivor is a complete progress snapshot. Producing the terminal event from the join result (rather than `finish()`/`abandon()` callbacks through the channel) also covers a panicking download task, which surfaces as an `error` event instead of a silently truncated stream. A client disconnect drops the receiver and the response body; the blocking download runs to completion and its staging cleanup still applies, so a retry hits the cache.
- Cost: a disconnected client does not stop the download (no mid-read cancellation path - the bytes land in the cache, which is arguably the useful outcome); a client slower than ~64 samples per download sees gaps in the byte series, never a wrong total.

## 57. DELETE /v1/cache/{sha256} matches on the sidecar digest

- Choice: the delete route validates the path parameter as a 64-hex digest (400 `malformed_request` otherwise), finds the entry by comparing against sidecar digests from the same walk the listing uses, and removes blob plus sidecar through the confinement-checked `remove_cache_entry`. Absent entries answer 404 with a new `cache_entry_not_found` envelope code; pre-commit storage failures are 500 `cache_error`.
- Evidence: keying delete by content digest (not by source URL or path) means the caller never supplies a path segment, so there is no traversal surface on the route at all - the digest is only ever compared against sidecar contents, never joined into a path; matching the sidecar (not re-hashing blobs) keeps delete consistent with the listing's Amendment C rule, so a blob whose bytes were swapped underneath its sidecar is still removable by its recorded digest.
- Cost: two entries sharing one digest (the same bytes fetched from two sources) are both legitimate cache entries, but delete removes only the first the walk finds - the second delete call removes the other; and a sidecar-less provisioning blob remains undeletable through the API, by design (entry 55).

## 58. The UI build is a Node.js requirement driven by build.rs

- Choice: the workbench UI moved from hand-written JS served by `include_str!` to TypeScript under `ui/src/`, bundled by esbuild into `ui/dist/` (gitignored) and served through `rust-embed` (`#[folder = "ui/dist/"]`: disk reads in debug, embedded in release). The server crate's `build.rs` runs the bundle on every cargo build that re-runs it - preferring `ui/node_modules/.bin/esbuild` (via `cmd /c` on Windows, whose npm shims are `.cmd` files) and falling back to `npx --yes esbuild` with a `cargo::warning` - then copies the static files (`index.html`, `style.css`, `pcm-worklet.js`, `markdown-it.min.js`) into `ui/dist/` itself. A failed esbuild run panics the build with the install instructions. `ui/package.json` carries `dockview` and `marked` as runtime dependencies and `esbuild` + `typescript` as dev dependencies; `npm run build` / `npm run watch` / `npm run typecheck` wrap the same steps for the fast iteration loop.
- Evidence: the plan's UI foundation decision fixes esbuild + rust-embed and demands that a plain `cargo build` always produces fresh JS, which only a build script can guarantee; putting the copy step in `build.rs` (rather than shelling out to `npm run build`) keeps the cargo path independent of npm's script runner and shaves npm's startup off every rebuild. The `npx` fallback keeps a fresh checkout buildable before `npm install`, at the cost of a registry fetch. esbuild 0.28.2 mis-parses a nested inline type literal inside an `as` cast (`(x as { choices?: Array<{ delta?: { content?: unknown }> } })` - "Unexpected >"), so casts of that shape are hoisted to named interfaces; esbuild only strips types, and `tsc --noEmit` remains the real checker.
- Cost: Node.js becomes a hard build requirement for the server crate where plain cargo used to suffice (mitigated by the panic message and the README's UI development section); `build.rs` writes into the source tree (`ui/dist/`), against the usual OUT_DIR rule, because rust-embed's debug mode reads a fixed manifest-relative path - the directory is gitignored and fully regenerated; and the static-file list lives in two places (`build.rs` and `ui/build.mjs`), cross-referenced by comments.
