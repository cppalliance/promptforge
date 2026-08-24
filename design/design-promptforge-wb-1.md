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
