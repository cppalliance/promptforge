# promptforge-wb-server

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](LICENSE)

The PromptForge Workbench HTTP server. It serves a local chat UI and API on loopback: an OpenAI-shaped model catalog and chat relay in front of a PromptForge gateway (with streaming over a WebSocket), a JSONL session tape recording every exchange, and a WebSocket voice endpoint that transcribes push-to-talk microphone audio on-device with whisper.cpp. The desktop shell (`promptforge-wb`) embeds it in-process; run standalone it is the browser-tab frame of the same workbench.

## Quick start

Create a `workbench.toml` in the current directory (see [`workbench.example.toml`](../../workbench.example.toml) at the repository root for a commented version of every field):

```toml
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"
```

Then run:

```bash
cargo run -p promptforge-wb-server
```

The server binds `127.0.0.1:7910` by default and serves the chat UI at `http://127.0.0.1:7910/`. Set `server.open_browser = true` to have it open your system browser once it is serving.

The desktop shell (`promptforge-wb`) is the zero-config path: it searches beside its executable, then the current directory, then `~/.promptforge/`, and on first run writes a default `workbench.toml` into `~/.promptforge/` and loads that. The server binary does not generate one - it reads `workbench.toml` from the current directory only.

String values support `${VAR}` environment interpolation; `$$` is a literal `$`, and an unset variable interpolates to the empty string.

## Configuration

Every field of `workbench.toml`:

| Field | Default | Description |
| --- | --- | --- |
| `gateway.base_url` | `http://127.0.0.1:8081` when empty | Base URL of the PromptForge gateway; an empty value (for example an unset `${PROMPTFORGE_GATEWAY_URL}`) falls back to the default |
| `gateway.api_key` | (empty) | Bearer key for the gateway API; supports `${VAR}` interpolation; empty sends no `Authorization` header |
| `tape.path` | `tape.jsonl` | Path of the JSONL session tape; one event per chat exchange |
| `server.bind` | `127.0.0.1:7910` | Address the workbench server binds to |
| `server.open_browser` | `false` | When true, the server binary opens the system browser at its address once serving; the desktop shell ignores it |
| `voice.interim_model` | (empty) | Path to the GGML whisper model for streaming interim transcription; empty disables transcription |
| `voice.final_model` | (empty) | Path to the whisper model for the pipelined final pass; empty falls back to the interim model |
| `voice.interim_source` | (empty) | URL the interim model can be downloaded from |
| `voice.final_source` | (empty) | URL the final-pass model can be downloaded from |
| `voice.window_seconds` | `5` | Seconds of trailing audio each interim pass transcribes |
| `voice.interval_ms` | `800` | Milliseconds between interim passes while a take is recording |

## Routes

| Route | Description |
| --- | --- |
| `GET /health` | Health probe; answers `{"status":"serving"}` |
| `GET /` | The chat UI (also `/app.js`, `/app.css`, `/style.css`, `/pcm-worklet.js`, served from `ui/dist/`: read from disk in debug builds, embedded in the binary in release builds) |
| `GET /v1/models` | Proxies the gateway's model catalog verbatim; while the gateway is known down, answers 502 `gateway_unreachable` without attempting it |
| `POST /chat` | Buffered chat relay: `{"model", "messages"}` in, gateway response out; `"stream": true` is rejected with 400 - streaming lives on `/ws`; while the gateway is known down, answers 502 `gateway_unreachable` without attempting it |
| `GET /ws` | WebSocket upgrade, one persistent socket for all downstream JSON: `{"type":"chat","id","model","messages"}` frames in (the optional `id` is echoed on the reply), `{"type":"delta","content"}` / `{"type":"done"}` / `{"type":"error","message"}` frames out, plus unsolicited `{"type":"status","label","description","severity","activity","progress"}` observer updates and `{"type":"models","models":[...]}` catalog pushes when the gateway comes back after an outage |
| `GET /voice` | WebSocket upgrade: binary f32 PCM at 16 kHz mono in, `start`/`stop` control words, interim and final transcripts out |

## Gateway resilience

A background heartbeat polls the gateway's `GET /health` every five seconds and reports transitions on the status bus: "Gateway unreachable" when the gateway stops answering, "Connected to gateway" when it comes back. While the gateway is known down, chat over `/ws` is answered immediately with a `{"type":"error","message":"Gateway unreachable"}` frame (no upstream attempt, nothing taped), and `GET /v1/models` and `POST /chat` answer 502 `gateway_unreachable` instead of waiting on a dead connection. A reconnect re-fetches the model catalog and pushes it to every `/ws` session as a `{"type":"models",...}` frame, so a UI that booted during the outage refreshes its model picker by itself. The server boots and serves the UI whether or not the gateway has ever answered; voice works whenever its models are local.

## UI development

The chat UI is TypeScript under `ui/src/`, bundled by esbuild into `ui/dist/app.js`. Node.js is required: run `npm install` in `ui/` once per checkout. After that, `cargo build` runs the UI build itself (the crate's `build.rs` prefers `ui/node_modules/.bin/esbuild` and falls back to `npx esbuild`, which may download esbuild on first use). `ui/node_modules/` and `ui/dist/` are gitignored.

Two workflows:

1. **Just cargo:** edit the TypeScript, then `cargo build` (or `cargo run -p promptforge-wb-server`). The build script re-bundles whenever `ui/src/` or the static UI files change, and debug builds read `ui/dist/` from disk on every request.
2. **esbuild watch:** run `npm run watch` in `ui/` in one terminal and `cargo run` in another. Edit, save, refresh the browser - no Rust recompile for UI changes.

`npm run typecheck` runs `tsc --noEmit`; esbuild strips types without checking them, so the typecheck is advisory. `npm test` runs a jsdom smoke test that imports the built `dist/app.js` and asserts the chat UI mounts (run `npm run build` first).

The chat UI itself is [murm-ui](https://github.com/levmv/murm-ui) 0.2.0, vendored in `ui/src/chat/` (MIT, see its `PROVENANCE.md`), driven by a WebSocket provider against `GET /ws` (one persistent socket, opened on load; chat frames carry an `id` the server echoes, and unsolicited status frames ride the same connection). Its styles are bundled by esbuild into `dist/app.css`; `ui/style.css` carries the workbench shell (sidebar, picker, voice UI, status bar) and overrides.

The status bar at the bottom of the window renders the observer's `{"type":"status",...}` frames (`ui/src/status-bar.ts`): the label as the bar text, the description as the tooltip, error frames in a distinct color. Debug-severity frames are internal instrumentation and never touch the text. The right slot holds a `<progress>` bar while a frame carries progress, and an activity LED otherwise: a small circle that pulses green on gateway traffic and amber on voice activity (green wins when both coincide), lit for one pulse window per frame and faded by a CSS transition. The bar's colors, glow radii, and pulse window are CSS custom properties (`--led-green`, `--led-amber`, `--led-off`, `--led-glow-radius`, `--led-pulse-ms`, `--progress-fill`, `--progress-glow`, ...) at the top of `ui/style.css`.

## Skinning

The whole UI skins from the `:root` block at the top of `ui/style.css` - every color, spacing step, radius, font, scrollbar metric, and the status bar's LED and progress effect is a CSS custom property there. The vendored murm-ui chat panel skins from the same block through a bridge in `ui/style.css` (the `.mur-app[data-theme="dark"]` rule, with a comment mapping each `--mur-*` variable to the workbench variable it follows).

Two ways to reskin:

1. **Edit the block.** Change values in the `:root` block of `ui/style.css` and rebuild (`cargo build`; debug builds serve `ui/dist/` from disk). This is the path for changes you keep.
2. **Override from an additional stylesheet.** Add a `<link>` after `/style.css` in `ui/index.html` and redeclare any variable on `:root`. Later declarations win the cascade, and because the murm-ui bridge dereferences the workbench variables at computed-value time, overriding e.g. `--bg` re-skins the chat panel too. To retune murm-ui-only knobs (`--mur-chat-form-width`, the shadows), target `.mur-app[data-theme="dark"]` in the same stylesheet.

The variables:

| Variable | Default | What it paints |
| --- | --- | --- |
| `--bg` | `#0d0e12` | Window and chat background |
| `--bg-raised` | `#14161c` | Raised surfaces (cards, code blocks) |
| `--bg-hover` | `#1a1d25` | Hover washes, user message bubble |
| `--bg-sidebar` | `--bg-raised` | Sidebar background |
| `--bg-composer` | `--bg` | Chat composer form |
| `--text` | `#d6d9e0` | Body text (13:1 on `--bg`) |
| `--text-muted` | `#8b90a0` | Dimmed text (6:1 on `--bg`; do not go dimmer, 4.5:1 is the floor) |
| `--border` | `#262a33` | Hairline borders |
| `--accent` | `#7c7fd4` | Primary action (send button) |
| `--accent-dim` | `#5658a0` | Focus border |
| `--danger` | `#b0606a` | Recording background, danger accents (non-text) |
| `--danger-text` | `#cf7f88` | Danger as text on dark surfaces |
| `--on-danger` | `#ffffff` | Icon or text on a `--danger` fill |
| `--font-prose` | system stack | UI font |
| `--code-font` | ui-monospace stack | Code blocks, code chrome |
| `--space-xs`..`--space-xl` | `4/6/8/12/16px` | Shell spacing scale |
| `--radius` | `6px` | Control corner radius |
| `--sidebar-width` | `220px` | Sidebar width |
| `--status-bar-height` | `24px` | Status bar height |
| `--status-bar-bg` | `--bg-raised` | Status bar background |
| `--status-bar-text` | `--text-muted` | Status bar text |
| `--status-bar-text-error` | `--danger-text` | Status bar error text |
| `--status-bar-padding-inline` | `--space-lg` | Status bar horizontal padding |
| `--status-bar-gap` | `--space-lg` | Status bar item gap |
| `--progress-width` | `96px` | Progress bar width (also the slot's minimum) |
| `--progress-height` | `6px` | Progress bar height (drives its rounding) |
| `--progress-fill` | `#4caf7d` | Progress fill |
| `--progress-track` | `rgba(255,255,255,0.08)` | Progress track |
| `--progress-glow` | `4px` | Blur radius of the fill's glow |
| `--led-size` | `10px` | Activity LED diameter |
| `--led-green` / `--led-amber` | `#4caf7d` / `#d9a03f` | Gateway / voice activity colors |
| `--led-off` | `rgba(255,255,255,0.08)` | The unlit LED lens |
| `--led-core` | `#ffffff` | Hot center of the lit gradient |
| `--led-glow-radius` | `6px` | Base blur of the layered bloom |
| `--led-pulse-ms` | `250ms` | Pulse hold window and fade-out (also read by the status bar's JS) |
| `--led-fade-in-ms` | `60ms` | Fade-in when a pulse lights the LED |
| `--led-lens-highlight` / `--led-lens-shadow` | white/black alphas | Idle lens inset shading |
| `--scrollbar-width` | `8px` | Scrollbar thickness (drives thumb rounding) |
| `--scrollbar-thumb` | `rgba(255,255,255,0.16)` | Scrollbar thumb |
| `--scrollbar-thumb-hover` | `rgba(255,255,255,0.28)` | Scrollbar thumb on hover |

## Whisper models

Whisper models are not downloaded by the build; fetch them out of band from the whisper.cpp GGML model collection on Hugging Face: <https://huggingface.co/ggerganov/whisper.cpp>. Production configs typically pair `ggml-large-v3-turbo.bin` (interim) with `ggml-large-v3.bin` (final). The test suite uses the tiny English model (`ggml-tiny.en.bin`) plus the `jfk.wav` speech fixture, placed in `tests/fixtures/` (gitignored); a missing fixture fails the test with the download URL in the message.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
