---
name: Auto-generate config on first run
overview: Config generation, gateway cache API, status bar with observer and activity LED, CSS skinning, and gateway resilience - all in one plan touching both the gateway and the workbench.
todos:
  - id: auto-config
    content: Auto-generate ~/.promptforge/workbench.toml on first run, remove from_env path
    status: pending
  - id: gateway-cache-api
    content: "Gateway: POST/GET/DELETE /v1/cache with SSE download progress"
    status: pending
  - id: observer
    content: Observer broadcast channel in AppState, status on main WS, instrument all subsystems
    status: pending
  - id: status-bar
    content: "Status bar UI: text left, progress bar right, activity LED, custom scrollbar"
    status: pending
  - id: css-skinning
    content: Consolidate all visuals into CSS custom properties, fix scrollbar, skinnable
    status: pending
  - id: gateway-resilience
    content: Background health heartbeat, reconnect, graceful degradation
    status: pending
  - id: wb-cache-integration
    content: Workbench calls POST /v1/cache at startup for whisper models, pipes progress to status bar
    status: pending
isProject: false
---

# Auto-generate config, gateway cache API, status bar, and skinning

This plan touches both the promptforge-gateway and the promptforge workbench crates. It assumes the stage-1 build is complete (commits through `cb00dee` on master in the promptforge repo at `c:\Users\Vinnie\cursor\promptforge`).

## Context for a fresh reader

The promptforge workspace (`c:\Users\Vinnie\cursor\promptforge`) contains:
- `crates/promptforge-gateway` - a local model gateway serving OpenAI-compatible chat completions, with local GGUF provisioning via hf-hub
- `crates/promptforge-wb-server` - an axum HTTP server: chat proxy to the gateway via WebSocket, append-only tape, push-to-talk voice with whisper-rs transcription, dark Cursor-like UI served as embedded static assets
- `crates/promptforge-wb` - a desktop shell (wry/tao window) that spawns the server in-process

The workbench currently requires a `workbench.toml` config file (or env vars) to start. Voice requires manually downloading whisper models. The UI has no status feedback, no activity indicator, and an ugly native scrollbar.

## Rules of engagement

- **Drive to completion.** Do not stop until every step is done or an error has no forward path.
- Follow `tools-public/rulebooks/vibe-rulebook.md`: work in subagents, one testable commit per step, coder then review-and-fix, verify on schedule.
- Follow `tools-public/rulebooks/rust-rulebook.md` for all Rust.
- Follow `tools-public/rulebooks/html-css-rulebook.md` for all UI work: semantic HTML, BEM-style classes, CSS custom properties, no inline styles, accessible contrast.
- When a build decision contradicts or extends the design notes, revise the notes in the same commit.
- Record every design choice in `design/design-promptforge-wb-1.md` (append, choice/evidence/cost format).
- Do NOT stop for user confirmation between steps. Fix forward.

## Step 1: Auto-generate config on first run

**What changes:**
- `crates/promptforge-wb/src/discover.rs`: when no TOML is found in the three-path search (exe dir, cwd, `~/.promptforge/`), create `~/.promptforge/` if missing, write `~/.promptforge/workbench.toml` with the default content below, log the path, and return it. The app never exits on missing config.
- `crates/promptforge-wb-server/src/config.rs`: remove the `from_env` / `from_env_lookup` functions added in commit `cb00dee`. The generated TOML uses `${VAR}` interpolation for the gateway fields - the existing interpolation machinery resolves them. If the env var is unset, interpolation resolves to empty string: empty `api_key` means no Authorization header is sent; empty `base_url` falls back to the hard default `http://127.0.0.1:8081`.
- Update both crate READMEs and `workbench.example.toml` header.

**The generated file (`~/.promptforge/workbench.toml`):**
```toml
# PromptForge Workbench configuration
# Generated on first run. Edit as needed.
# See: crates/promptforge-wb-server/README.md

[gateway]
base_url = "${PROMPTFORGE_GATEWAY_URL}"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"

[server]
bind = "127.0.0.1:7910"
# open_browser = false

[tape]
path = "tape.jsonl"

[voice]
# Download from: https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin
interim_model = "~/.promptforge/models/ggml-large-v3-turbo.bin"
# Download from: https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin
final_model = "~/.promptforge/models/ggml-large-v3.bin"
window_seconds = 5
interval_ms = 800
```

**Commit decomposition:**
1. Remove `from_env` / `from_env_lookup` from config.rs; adjust config error to distinguish "file not found" from "parse error". Test: parse-error still fails; file-not-found returns a typed variant.
2. `discover.rs` returns `Option<PathBuf>` on miss (already done) plus a new `generate_default(path) -> io::Result<PathBuf>` that writes the template; the shell calls it when discover returns None. Test: tempdir with no TOML -> generates file -> file contents match template.
3. Config loader: empty `base_url` after interpolation falls back to the hard default; empty `api_key` means no auth header. Test: config from the generated template with no env vars loads successfully; gateway client sends no auth header.
4. Update READMEs, `workbench.example.toml` header, and decision log.

## Step 2: Gateway cache API

Add to `crates/promptforge-gateway`:

**Routes (bearer-authenticated):**
- `POST /v1/cache` - body: `{"source": "<HF URL>", "sha256": "<optional>"}`. If already cached: returns `{"path": "...", "status": "ready"}`. If not cached: responds with an SSE stream - `data: {"status": "downloading", "bytes": N, "total": N}` events, terminated by `data: {"status": "ready", "path": "..."}`.
- `GET /v1/cache` - returns `[{"source", "path", "sha256", "size_bytes"}, ...]`.
- `DELETE /v1/cache/{sha256}` - removes from disk and listing.

**Implementation:** the download logic already exists in the gateway's local-model provisioning (`hf-hub` crate from Hugging Face, progress via `indicatif`, SHA256 verification). Refactor into a reusable internal function, expose through the new routes. Cache directory: the gateway's existing `[local].cache_dir` (default `~/.promptforge`). No new config fields.

**Commit decomposition:**
1. Refactor the gateway's existing download logic (from `local_runtime.rs` or equivalent) into a standalone `cache` module with `download_to_cache(source, sha256, cache_dir, progress_tx) -> Result<PathBuf>`. Test: unit test with a mock HTTP server serving a small file; assert download, SHA verify, and path.
2. `GET /v1/cache` route: lists cached files from the cache directory. Test: seed two files; assert listing returns both with correct metadata.
3. `POST /v1/cache`: if cached, return immediately; if not, stream SSE progress from the download function. Test: mock server; assert SSE events arrive with bytes/total; assert final "ready" with path; assert file on disk.
4. `DELETE /v1/cache/{sha256}`: removes the file. Test: seed a file; delete; assert gone from listing and disk.

## UI foundation decision (applies to steps 3-5)

The UI migrates from vanilla JS to TypeScript, bundled with esbuild (single-command, <50ms builds), served via `rust-embed` (debug: filesystem, release: embedded in binary).

**Dependencies:**
- **dockview** (npm, MIT, zero-dep) - the layout engine: tabs, resizable splits, drag-and-drop docking, floating panels, layout serialization. Used as a proper dependency to pick up upstream fixes.
- **murm-ui** (vendored, MIT) - vanilla TypeScript chat UI for LLMs, forked into the project. Provides the streaming chat renderer, throttled DOM updates, message bubbles, auto-scroll, and theme infrastructure. Modified to use a WebSocketProvider instead of fetch, integrated with the observer, restyled to match the workbench palette.
- **esbuild** (dev dependency) - bundles `ui/src/*.ts` into one `app.js` that gets `include_str!`-ed into the server binary.

**Layout architecture (starts simple, grows with stages):**
```
dockview (panel engine)
├── Panel: Chat (murm-ui, adapted)
├── Panel: File tree (future)
├── Panel: Block editor (future)
├── Panel: Store filesystem (future)
└── (below dockview) Status bar - always visible, not a panel
```

In this plan, the dock has one panel (the chat) filling the whole window. It looks identical to the current UI but the panel infrastructure is there - adding panels later is `dockview.addPanel(...)`, not a rearchitecture.

**Build pipeline:**
```
ui/src/*.ts → esbuild --bundle → ui/dist/app.js → rust-embed serves from disk (debug) or embeds (release)
```

**Asset serving:** `rust-embed` crate with `#[derive(Embed)] #[folder = "ui/dist/"]`. In debug: reads from the filesystem (live reload). In release: embedded in the binary. Replaces the current `include_str!` approach and gives dev-mode live reload for free.

**build.rs** in the server crate: calls `npx esbuild ui/src/main.ts --bundle --outdir=ui/dist` with `rerun-if-changed=ui/src`. This means a plain `cargo build` always produces fresh JS - you never have to run npm manually. But for fast UI iteration, run esbuild in watch mode in a separate terminal and skip Rust rebuilds entirely.

**Two dev workflows (documented in the crate README):**
1. **Simple (just cargo):** `cargo run -p promptforge-wb` - build.rs runs esbuild, rust-embed reads the output from disk. Edit TS, `cargo build` again. Works with no npm knowledge.
2. **Fast (esbuild watch):** `npx esbuild --watch ui/src/main.ts --bundle --outdir=ui/dist` in one terminal, `cargo run` in another. Edit TS, save, refresh browser. No Rust recompile for UI changes.

**package.json** at `crates/promptforge-wb-server/ui/`: `dockview`, `esbuild` (dev), and `typescript` (dev, for type checking only - esbuild handles emit).

## Step 3: Migrate chat from SSE to WebSocket, add observer and status

This step is a significant refactor. The map below guides the coder.

### Refactoring map: SSE to WebSocket

**Architecture change:** the browser-to-workbench chat transport moves from `POST /chat` with `text/event-stream` to `GET /ws` WebSocket upgrade with bidirectional JSON text frames. The gateway upstream (reqwest SSE via `SseDecoder`) is UNCHANGED.

**New `/ws` route follows the existing `/voice` pattern:**
- `WebSocketUpgrade` extractor + `ws.on_upgrade(...)` in a `get(...)` route
- `socket.split()` into sink + stream
- Outbound `mpsc::channel<Message>(32)` + dedicated writer task
- Inbound `Message::Text` JSON parsing in a `while let Some(received) = stream.next().await` loop
- Session id via `AtomicU64`

**JSON protocol (text frames only):**

Client to server:
```json
{"type":"chat", "model":"...", "messages":[...]}
```

Server to client:
```json
{"type":"delta", "content":"..."}
{"type":"done"}
{"type":"error", "message":"..."}
{"type":"status", "label":"...", "description":"...", "severity":"info", "activity":"gateway", "progress":null}
```

**Files that change:**

| File | What to do |
|---|---|
| `src/app.rs` | Add `.route("/ws", get(chat_ws::upgrade))`. Remove `chat_stream()` (lines ~196-266), `wants_stream()`, `Sse`/`Event` imports, `stream::unfold` SSE relay. Keep `chat()` POST for non-streaming only (reject `stream:true` with 400). Keep `delta_content()`, `tape_round_trip()`, `StreamTape` - move to shared if needed. |
| `src/chat_ws.rs` (new) | `upgrade()`, `run_session()`: on inbound chat JSON, call `gateway.chat_completion_stream()`, poll `SsePayloadStream`, send `{type:delta}` frames per payload, `{type:done}` at end, tape via `StreamTape::record()`. On `ChatStream::Relay` (gateway declined): send `{type:error}` frame. Status updates via observer. |
| `src/lib.rs` | Add `mod chat_ws;`, update re-exports. |
| `ui/app.js` | Replace `send()` streaming path (lines ~106-124): open WS to `/ws`, send JSON chat frame, receive delta/done/error frames. Delete `streamInto()` (lines ~138-179). Keep voice WS, markdown rendering, history management. |
| `src/gateway.rs` | **No changes.** `SseDecoder`, `ChatStream`, `payload_stream()` all stay - they decode the gateway upstream, not the browser leg. |
| `src/tape.rs` | **No changes.** Same schema, same writer. |
| `src/voice.rs` | **No changes.** Reference pattern only. |
| `README.md` | Update route table: `/ws` streaming, `/chat` buffered only. |

**What to remove:**
- `chat_stream()` handler function
- `wants_stream()` helper
- `Sse`, `Event` imports from `axum::response::sse`
- `stream::unfold` SSE relay code
- Client `streamInto()` function
- All SSE-specific tests (replace with WS equivalents)

**What to keep/repurpose:**
- `SseDecoder` + `SsePayloadStream` (decodes gateway, not browser)
- `StreamTape` (same assembly logic, triggered from WS session end)
- `delta_content()` (extracts content from gateway SSE JSON)
- `tape_round_trip()` (called after stream completes)
- Gateway mock handlers (they mock the upstream SSE, not the browser)

**Test migration:**
- Use `tokio-tungstenite` (already a dev-dep) to open WS to `/ws`
- Send JSON chat text frame
- Assert delta frames arrive in order, followed by done
- Assert one tape event with assembled response
- Assert mid-stream gateway error produces error frame + tape with error note
- Assert declined stream (gateway non-2xx) produces error frame

**Commit decomposition for step 3:**
1. Set up the TS build pipeline: `package.json` with dockview + esbuild + typescript; `tsconfig.json`; esbuild script; verify `npm run build` produces `ui/dist/app.js`. Update `include_str!` path in Rust. Test: `cargo build` still green with new asset path.
2. Vendor murm-ui source into `ui/src/chat/`; adapt imports for the TS build; verify it compiles under esbuild. Test: the bundled app.js loads in the browser and shows the chat.
3. Initialize dockview with one panel (chat); murm-ui renders inside the panel. Status bar placeholder div below. Test: visual (the chat still works as before, now inside a dockview panel).
4. Add the `/ws` WebSocket route (`chat_ws.rs`): upgrade handler, session loop, inbound chat JSON dispatch, gateway stream relay as `{type:delta}` frames, `{type:done}` on completion, `{type:error}` on failure. Tape recording after stream. Test: tokio-tungstenite client sends chat, receives delta frames in order + done, tape has one event.
5. Wire murm-ui's provider to the `/ws` WebSocket instead of fetch. Remove `streamInto()` and the old SSE fetch path from the TS. Test: visual (chat streams via WS); server test (WS mid-stream error produces error frame + tape with error note).
6. Remove dead SSE code from `app.rs`: `chat_stream()`, `wants_stream()`, `Sse`/`Event` imports, old SSE tests. `POST /chat` rejects `stream:true` with 400. Test: clippy + tests green; `POST /chat` with `stream:true` returns 400.
7. Add the `status` module: `StatusBarUpdate` struct, `Severity`, `Activity`, `Progress`; broadcast channel in `AppState`; status frames sent on `/ws` as `{type:status,...}`. Test: subscribe to /ws, emit a status update programmatically, assert it arrives as a JSON frame.
8. Instrument all subsystems with observer calls (stubs): chat submit/stream/done, voice listen/transcribe/final, startup phases, errors. Test: trigger chat over WS, assert at least one status frame with "Submitting" arrives.

### Observer integration (details above, executed in commits 7-8)

Add to `crates/promptforge-wb-server`:

**The observer.** A tokio broadcast channel of `StatusBarUpdate` held in `AppState`:
```rust
pub struct StatusBarUpdate {
    pub label: String,            // short text rendered in the status bar
    pub description: String,      // longer text shown as tooltip on hover
    pub progress: Option<Progress>,
    pub severity: Severity,
    pub activity: Activity,
}
pub struct Progress { pub current: u64, pub total: u64 }
pub enum Severity { Info, Debug, Error }
pub enum Activity { General, Gateway, Voice }
```

Status events stream on the main WebSocket `/ws` as `{"type":"status",...}` frames (no separate endpoint).

**Instrument all subsystems (stubs - labels will be tuned later):**
- Startup: "Loading configuration" -> "Loading whisper model" -> "Connecting to gateway" -> "Ready"
- Gateway connectivity: "Connected to gateway" / "Gateway unreachable" / "Reconnecting..."
- Model catalog: "Loading models..." -> idle
- Chat (non-streaming): "Submitting request..." -> "Waiting for response..." -> idle
- Chat (streaming): "Submitting request..." -> "Streaming response..." -> idle
- Voice recording: "Listening..."
- Voice interim transcription: "Transcribing..."
- Voice final pass: "Finalizing transcript..." -> idle
- Model cache download: "Downloading {filename} ({bytes}/{total})" -> "Download complete"
- Errors: "Gateway error: {status}" / "Transcription failed" / "Connection lost"

Activity enum: every gateway request/response chunk fires `Activity::Gateway`; every mic frame or transcription event fires `Activity::Voice`.

Internal debug-level hooks (~170 points across the codebase) also call the observer at `Severity::Debug`; the UI ignores those.

**Tests:** connect to /ws, trigger a chat via the socket, assert at least one `{type:status}` frame with label containing "Submitting" arrives.

## Step 4: Status bar UI

In `crates/promptforge-wb-server/ui/`:

- A permanent status bar docked at the bottom of the window (below the composer)
- Left: text string from the observer
- Right: either the progress bar OR the activity LED - they are mutually exclusive and occupy the same space. The progress bar takes priority: when progress is non-None, the bar renders (green fill with subtle glow) and the LED is hidden. When progress is None, the LED renders: a small circle (~10px) with a realistic LED look: when idle, a dark translucent disc with a subtle inner highlight (like an unlit LED lens). When active, it glows - a radial gradient center (bright white-green or white-amber) with a soft `box-shadow` bloom spreading outward (2-3 layered shadows at increasing blur radii for the halo effect). The glow fades in quickly and fades out over ~250ms with an ease-out curve. Green for `Activity::Gateway`, amber for `Activity::Voice`. Green wins on collision. All colors and glow radii are CSS variables (`--led-green`, `--led-amber`, `--led-glow-radius`, `--led-pulse-ms`) so the effect is tunable without touching JS.

The JS connects to `/ws` on load; status frames update the bar, chat delta frames update the chat. One connection for all JSON downstream.

**Commit decomposition:**
1. Status bar HTML/CSS: permanent bar below the dockview container; left text, right slot (progress or LED); BEM classes; CSS custom properties for all values. Test: visual (bar renders with placeholder text).
2. Status frame rendering: JS subscribes to `{type:status}` on the WS and updates the bar text + tooltip. Test: visual + the server test from step 3 commit 8 still passes.
3. Progress bar: renders when `progress` is non-null; green fill with glow animation; CSS variables. Test: visual (mock a download-like status update).
4. Activity LED: the realistic glow circle; idle/green/amber states; 250ms pulse via CSS transition; shown only when progress is null. Test: visual (trigger gateway and voice activity).

## Step 5: CSS skinning pass

Consolidate all visuals in `style.css`:
- Move every color, spacing, radius, font, scrollbar width, LED timing, and progress glow into CSS custom properties at the top of the file
- Custom scrollbar: thin (8px), rounded, translucent thumb, transparent track, no native chrome (`::-webkit-scrollbar` pseudoelements - WebView2 is Chromium)
- No inline styles anywhere in HTML
- All elements addressed by BEM-style classes
- Variables include at minimum: `--bg`, `--bg-sidebar`, `--bg-composer`, `--border`, `--accent`, `--text`, `--text-muted`, `--code-font`, `--led-green`, `--led-amber`, `--led-off`, `--led-pulse-ms`, `--scrollbar-width`, `--scrollbar-thumb`, `--progress-glow`

**Commit decomposition:**
1. Consolidate all colors, spacing, radii, fonts into CSS custom properties at the top of the stylesheet; remove any inline styles; rename classes to BEM. Test: visual (nothing changes on screen); `cargo build` green.
2. Custom scrollbar: webkit pseudoelements, thin, rounded, translucent thumb. Test: visual.
3. Document the variable list and skinning instructions in README; decision log entry.

## Step 6: Gateway resilience

In `crates/promptforge-wb-server`:
- A background task polls `GET /health` on the gateway every 5 seconds (interval configurable later).
- On failure: observer emits "Gateway unreachable" (Info, Activity::Gateway). Subsystems that depend on the gateway (chat, model catalog, cache) return user-visible errors in the UI but never crash.
- On reconnection: observer emits "Connected to gateway". The model catalog is refreshed automatically.
- If the gateway was never reachable at startup, the app still starts - chat shows the error, voice works if models are local, the status bar says what's wrong.

**Commit decomposition:**
1. Background heartbeat task: spawns on server start, polls `GET /health` every 5s, emits status updates via observer ("Connected" / "Unreachable"). Test: mock gateway returns 200 -> observer fires "Connected"; mock returns error -> fires "Unreachable".
2. Model catalog auto-refresh on reconnection: when heartbeat transitions from unreachable to connected, re-fetch `/v1/models` and push a `{type:models,...}` frame on the WS. Test: gateway drops then comes back; assert models frame arrives on the WS after reconnection.
3. Graceful degradation: chat and catalog routes return user-visible error frames (not crashes) when the gateway is unreachable. Test: gateway down; send chat via WS; assert `{type:error}` frame with "Gateway unreachable".

## Step 7: Workbench calls cache API for whisper models

At startup, after the gateway heartbeat reports connected:
- Call `POST /v1/cache` for each whisper model pin from config (interim_model URL, final_model URL).
- Pipe the download progress through the observer -> main WS -> status bar.
- On ready: load the models from the returned paths.
- On failure (gateway unreachable, download error): voice stays disabled, status bar shows why, app continues.

**Commit decomposition:**
1. Startup cache calls: after heartbeat reports connected, call `POST /v1/cache` for each configured whisper model URL. On "ready" response, store the path. On failure, log and disable voice. Test: mock gateway cache returning "ready" immediately; assert model paths stored.
2. Pipe download progress to the observer: when the cache response is an SSE download stream, forward bytes/total to the observer as status updates with progress. Test: mock returning a short download stream; assert progress status frames arrive on the main WS.
3. Load whisper models from the cached paths: after both cache calls succeed, initialize the voice engine from the returned paths instead of from config paths. Test: end-to-end with mock cache returning paths to the existing test fixtures; assert transcription works.

## Key paths

- Workspace root: `c:\Users\Vinnie\cursor\promptforge`
- Gateway crate: `crates/promptforge-gateway/`
- Workbench server: `crates/promptforge-wb-server/`
- Workbench shell: `crates/promptforge-wb/`
- UI assets: `crates/promptforge-wb-server/ui/`
- Decision log: `design/design-promptforge-wb-1.md`
- Rulebooks: `c:\Users\Vinnie\cursor\tools-public\rulebooks/` (vibe-rulebook.md, rust-rulebook.md, html-css-rulebook.md)


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: auto-generate_config_on_first_run_9e22a89f

## Why this plan exists

This is the PromptForge workbench stage-2 plan, created 2026-08-24 in the creator chat right after stage 1 (window + mic, commits through cb00dee) first ran on the user's machine. Three frustrations from that first live session drove it:

1. **Config friction.** Stage 1 required env vars or a hand-written TOML. The user: "no way there are too many environment variables. there should not be any. the gateway I guess is a necessary evil but that's it... the tool should create the .toml file for the user, in their user directory... and it should have defaults for everything". And the decisive analogy: "can you imagine if you install Photoshop.exe and you have to set some env vars first in order to run it? no way." This reversed an earlier stage-1 decision - the user had said "the toml should not be needed if the env vars are set", which produced the `from_env` functions in cb00dee that this plan deletes.

2. **Whisper model downloads.** The user refused to duplicate download logic in the workbench: "I don't wanna duplicate this code... the gateway should have an API where someone can say, 'Hey, I need this hugging face pin'" (paraphrase of a longer dictation). The concrete reason: "I don't put my models on my system drive. I have a sixty terabyte drive that's separate for the models" - the gateway already owns the model-directory config, so the gateway should own caching.

3. **No UI feedback.** "that bar should always show the user like what the fuck the workbench is doing" - the status bar and observer came from wanting visibility into every subsystem. Separately, "the scroll bar on that browser control is fucking ugly" drove the CSS skinning pass.

## Design decisions and their origins

- **Generated config path and shape.** The user first said "the file should be promptforge-wb.toml and we might as well just put the {$PROMPTFORGE_GATEWAY_URL} interpolation in the config file", then "if we are going with a .promptforge directory then the config can be workbench.toml". Env interpolation inside the generated TOML replaced the from_env path - one mechanism instead of two. "include the defaults for the voice models, huggingface pins".
- **Cache API naming.** An "ensure this pin" endpoint was proposed; the user rejected the framing: "instead of 'ensure', which sounds like a product for geriatrics to get their vitamins, how about framing it as 'cache this model', 'list cached models', and so on". Hence POST/GET/DELETE /v1/cache.
- **Progress events.** "the gateway reports the total bytes needed, and the number of bytes it has downloaded thus far" - hence the SSE bytes/total events.
- **Resilience.** "the workbench must tolerate losing the connection to the gateway, and reconnecting".
- **Observer.** "I don't want a global variable, but I want something threaded through the entire program" - became the broadcast channel in AppState. Labels were expected to be tuned later: "put stubs if you have to, put something, put some text, and then I'll change it later".
- **Activity LED.** The metaphor is a router/ethernet activity light: "It's like a light on a router... the little green LED that shows you that there's activity". The user first wanted two LEDs (green gateway, amber voice), then merged them: "we combine them, we have Green and amber, and it can change color" - the plan took the combined single LED. Priority rule: "the progress bar and the LED are mutually exclusive, the progress bar takes priority". Aesthetic bar: "I want that LED to be beautiful with a soft glow like a real LED". Pulse timing (~250ms) was explicitly left for the user to tune.
- **StatusBarUpdate shape.** User-driven via rapid questions: add "General" to the enum instead of Option<Activity>; "pub label: String and pub description: String, the label is what you see in the status bar and the description is the tooltip bubble on hover"; the name "StatusBarUpdate" itself was the user's suggestion.
- **Transport.** The user asked "should the server's web app use websocket instead of SSE" and said "2 websocket connections is making sense to me and they are both two-way". During plan writing this converged to a single /ws socket for all downstream JSON; SSE survived only for the cache download, where the flow is strictly one-way.
- **UI stack.** "I want that javascript code to be clean and legible. should we be using typescript?" After a prior-art search: "that murm-ui looks beautiful, the demo has exactly the featureset that step 1 of our workbench needs" - vendored (forked) rather than a dependency, while dockview stayed "a proper dependency so I can pick up their upstream changes", chosen because "eventually I am going to want resizable panels, a tabbed dock for editing text files, a custom pane type to show the prompt visually".
- **Build pipeline.** Hard requirement that plain cargo build works with zero npm knowledge: "if I never use the esbuild can I still use cargo build to update the executable with changes to the typescript?" - hence build.rs invoking esbuild and rust-embed reading from disk in debug.
- **Voice model defaults (carried into the generated config).** "Low latency is important but it is more important to get the transcription right, early on, and so the user can see the words forming and correcting" - hence large-v3-turbo for interims, large-v3 for finals. Crash isolation for in-process whisper was explicitly rejected: "I dont care about the crash isolation. If there's a bug, they can fix it."

## Discarded alternatives

- Env vars as primary config (the from_env path) - reversed; see above.
- "Ensure"-style provisioning API - renamed to cache verbs.
- Two separate status LEDs - merged into one color-changing LED.
- Two WebSocket connections - collapsed to one /ws.
- murm-ui as an npm dependency - vendored instead so it could be rewritten (WebSocket provider, observer integration, restyle).
- Workbench-side model download - moved to the gateway to avoid duplicating download code and to reuse the gateway's configured cache directory.

## Run deviations (run chat 1717fa6d, 2026-08-24)

(Run chat 59952dc1 never executed this plan - it only cataloged transcripts in passing - so it contributed no deviations.)

- **Wrong assumption corrected at kickoff.** The plan says the gateway's download logic uses hf-hub; the run found otherwise and the user flagged it: "Gateway GGUF provisioning is reqwest + indicatif + sha2, not hf-hub (hf-hub lives in tool-picker)."
- **The observer needed an explicit idle/clear event.** The status bar got stuck: "the status is stuck on 'Finalizing transcript.' we probably need a 'status clear' event".
- **LED semantics changed mid-run.** "Change it. I want amber to mean that a model turn is processing. And green when there is a spurt of output tokens" - replacing the plan's green=gateway / amber=voice mapping.
- **A REC indicator was added that the plan never specified.** "I want a REC light. the word REC in caps... this takes priority whenever the mic is on... when REC is on, the green/amber LEDs are not shown"; later refined to maroon text in a rectangle, red with a 1px glow while recording. The status bar was also made a full-width top-level element.
- **GPU was an unstated assumption.** Whisper ran on CPU until CUDA was installed: "wait what? Why are we using CPU? This is supposed to use GPU ! JEsus".
- **The interim/final window transcription proved too volatile in live dictation** - text visibly shrank and reappeared on stop. This spun off a follow-on progressive-transcription design (stable crystallized prefix via speculative large-model passes, volatile tail), with a latch because "we need to make sure that we are never trying to do 2 final passes at the same time".
- **Process deviations.** For plan edits the user overrode the one-commit-per-step rule: "I want just one commit so make all the plan changes in once"; and later "run the plan without fuss. no subagents."
- **CI needed fixture gating.** Whisper-dependent tests failed in CI where model fixtures were absent; they were marked ignored, and one reconnect test was flaky under its deadline.
