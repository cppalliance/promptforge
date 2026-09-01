# promptforge-workshop-server

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](LICENSE)

The PromptForge Workshop HTTP server. It serves a local UI and API on loopback: agent sessions (chat runs through `.lua` agent programs over `workshop-agent`), an OpenAI-shaped model catalog passthrough in front of a PromptForge gateway, and workspace APIs. The gateway-owned `promptforge-stt` runtime attaches `/voice` when this server is embedded. The desktop shell (`promptforge-workshop`) embeds it in-process; run standalone it is the browser-tab frame without STT.

## Quick start

Create a `workshop.toml` in the current directory (see [`workshop.example.toml`](../../workshop.example.toml) at the repository root for a commented version of every field):

```toml
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"
```

Then run:

```bash
cargo run -p promptforge-workshop-server
```

The server binds `127.0.0.1:7910` by default and serves the chat UI at `http://127.0.0.1:7910/`. Set `server.open_browser = true` to have it open your system browser once it is serving.

The desktop shell (`promptforge-workshop`) is the zero-config path: it searches beside its executable, then the current directory, then `~/.promptforge/`, and on first run writes a default `workshop.toml` into `~/.promptforge/` and loads that. The server binary does not generate one - it reads `workshop.toml` from the current directory, or `workbench.toml` there if the canonical name is missing.

String values support `${VAR}` environment interpolation; `$$` is a literal `$`, and an unset variable interpolates to the empty string.

## Configuration

Every field of `workshop.toml`:

| Field | Default | Description |
| --- | --- | --- |
| `gateway.base_url` | `http://127.0.0.1:8081` when empty | Base URL of the PromptForge gateway; an empty value (for example an unset `${PROMPTFORGE_GATEWAY_URL}`) falls back to the default |
| `gateway.api_key` | (empty) | Bearer key for the gateway API; supports `${VAR}` interpolation; empty sends no `Authorization` header |
| `server.bind` | `127.0.0.1:7910` | Address the workshop server binds to |
| `server.open_browser` | `false` | When true, the server binary opens the system browser at its address once serving; the desktop shell ignores it |
| `server.state_dir` | the config file's directory | Directory holding the server's persistent state: agent session event logs live under `state_dir/sessions/`, and the per-profile model memory is written here |
| `agents.path` | `agents/` beside the config file | Directory whose `.lua` files are the launchable agent programs; a missing directory offers no agents |

## Routes

| Route | Description |
| --- | --- |
| `GET /health` | Health probe; answers `{"status":"serving"}` |
| `GET /` | The chat UI (also `/app.js`, `/app.css`, `/style.css`, `/pcm-worklet.js`, served from `ui/dist/`: read from disk in debug builds, embedded in the binary in release builds) |
| `GET /v1/models` | Proxies the gateway's model catalog verbatim; while the gateway is known down, answers 502 `gateway_unreachable` without attempting it |
| `GET /ws` | WebSocket upgrade, one persistent socket for the workshop's downstream JSON: unsolicited `{"type":"status","label","description","severity","activity","progress"}` observer updates, `{"type":"models","models":[...]}` catalog pushes, and `{"type":"workbench",...}` Model-menu snapshots out; `{"type":"select_model","model"}` and `{"type":"switch_profile","name"}` menu events in, refusals answered with `{"type":"error","message"}` frames |
| `GET /agents/ws` | WebSocket upgrade for one agent session: the discovered agent list on connect, `{"type":"launch","agent"}` / `{"type":"attach","session"}` in (acknowledged with `{"type":"agent_session","session","agent"}`), then durable `{"type":"agent_event","index","event",...}` log entries, ephemeral `{"type":"agent_delta","kind","content","reply"}` streaming chunks, and the `input_required` / `input_cancelled` wait frames answered by `{"type":"input_response","token","text"}`; `{"type":"cancel"}` fires turn-cancel |

## Gateway resilience

A background heartbeat polls the gateway's `GET /health` every five seconds and reports transitions on the status bus: "Gateway unreachable" when the gateway stops answering, "Connected to gateway" when it comes back. While the gateway is known down, `GET /v1/models` answers 502 `gateway_unreachable` instead of waiting on a dead connection, and the Model menu's `chat_ready` reads false. A reconnect re-fetches the model catalog and pushes it to every `/ws` session as a `{"type":"models",...}` frame, so a UI that booted during the outage refreshes its model picker by itself. The server boots and serves the UI whether or not the gateway has ever answered.

## UI development

The chat UI is TypeScript under `ui/src/`, bundled by esbuild into `ui/dist/app.js`. The bundled `ui/dist/` artifact is checked into the repository, so building the crate needs no Node.js - only changing the UI does. To work on the UI, Node.js is required: run `npm install` in `ui/` once per checkout. After that, debug `cargo build` runs the UI build itself (the crate's `build.rs` prefers `ui/node_modules/.bin/esbuild` and falls back to `npx esbuild`, which may download esbuild on first use). Without a local `ui/node_modules`, builds serve the checked-in artifact verbatim. `ui/node_modules/` is gitignored.

Release builds embed a verified, minified artifact: `build.rs` checks `ui/dist/manifest.json` (schema version, minified flag, a sha256 over every build input, and the dist file list) and, when the manifest is absent or stale against the current sources, produces the artifact itself by running `node build.mjs --package` in `ui/` (the same command as `npm run package`) before verifying and embedding. A single `cargo build --release` is sufficient, including after UI edits and after a debug build wiped `ui/dist/`; the build fails with instructions only when the artifact cannot be produced (for example Node.js or `ui/node_modules` missing) or still does not verify.

Two workflows:

1. **Just cargo:** edit the TypeScript, then `cargo build` (or `cargo run -p promptforge-workshop-server`). The build script re-bundles whenever `ui/src/` or the static UI files change, and debug builds read `ui/dist/` from disk on every request.
2. **esbuild watch:** run `npm run watch` in `ui/` in one terminal and `cargo run` in another. Edit, save, refresh the browser - no Rust recompile for UI changes.

`npm run typecheck` runs `tsc --noEmit`; esbuild strips types without checking them, so the typecheck is advisory. `npm test` runs `node --test`, which discovers every test under `ui/test/` plus any colocated `src/**/*.test.mjs` files; the suite includes a jsdom smoke test that imports the built `dist/app.js` and asserts the workbench mounts (run `npm run build` first).

The chat surface is the agent-session panel (`ui/src/ui/agent-session-view.ts`), rendered from the durable event stream over `GET /agents/ws`. `ui/style.css` carries the workshop shell (tree, panels, voice UI, status bar) and overrides.

The status bar at the bottom of the window renders the observer's `{"type":"status",...}` frames (`ui/src/ui/status-bar.ts`): the label as the bar text, the description as the tooltip, error frames in a distinct color. Debug-severity frames are internal instrumentation and never touch the text. The right slot holds a `<progress>` bar while a frame carries progress, and an activity LED otherwise: a small circle that pulses green on gateway traffic and amber on voice activity (green wins when both coincide), lit for one pulse window per frame and faded by a CSS transition. The bar's colors, glow radii, and pulse window are CSS custom properties (`--led-green`, `--led-amber`, `--led-off`, `--led-glow-radius`, `--led-pulse-ms`, `--progress-fill`, `--progress-glow`, ...) at the top of `ui/style.css`.

## Skinning

The whole UI skins from the `:root` block at the top of `ui/style.css` - every color, spacing step, radius, font, scrollbar metric, and the status bar's LED and progress effect is a CSS custom property there.

Two ways to reskin:

1. **Edit the block.** Change values in the `:root` block of `ui/style.css` and rebuild (`cargo build`; debug builds serve `ui/dist/` from disk). This is the path for changes you keep.
2. **Override from an additional stylesheet.** Add a `<link>` after `/style.css` in `ui/index.html` and redeclare any variable on `:root`. Later declarations win the cascade.

The variables:

| Variable | Default | What it paints |
| --- | --- | --- |
| `--bg` | `#0d0e12` | Window and chat background |
| `--bg-raised` | `#14161c` | Raised surfaces (cards, code blocks) |
| `--bg-hover` | `#1a1d25` | Hover washes, user message bubble |
| `--bg-sidebar` | `--bg-raised` | Sidebar background |
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

## Run event log

`WorkshopObserver` is the crate's append-only run event log. The `Observer` content hooks append runtime events (the write side), the `EventLog` trait serves indexed reads (the read side), and `subscribe()` broadcasts every appended entry live. Given a persist path it appends each event as one JSONL line behind a versioned header line; `load_from` replays such a file - refusing headers and lines it does not speak - and continues appending to it. A committed fixture in the crate's integration tests pins the version-1 file format against schema drift.

## Agent input waits

`WaitRegistry` holds an agent session's unresolved user-input waits behind single-use cryptographic tokens, retained across socket loss and resent on reconnect. `UserInputTool` is the Workshop's `user_input` tool - never advertised to a model - whose `call()` registers a wait, pushes the durable `input_required` frame itself, and suspends until `deliver_input_response` fires `on_user_input` byte-exact and completes the wait; its output is trusted, structured JSON (`text` byte-exact, `images` present and empty). A drop guard turns every dying wait into a durable `input_cancelled` frame, so a cancelled turn never leaks a wait or leaves a stale prompt.

## Agent sessions

`AgentSessions` (reached through `AppState::agents`) is the registry behind `GET /agents/ws`: it discovers `.lua` agent programs from `agents.path`, launches each as a session running `workshop_agent::run_agent` with the Workshop's `user_input` tool, a persisting `WorkshopObserver` event log at `state_dir/sessions/<session-id>.jsonl`, a model catalog built from the retained gateway catalog, and a `ui()` snapshot serving the selected model and the first granted workspace root. Sessions survive socket disconnect: sockets attach and detach, a reconnect replays the persisted log (every durable frame carries its log index) and re-announces unresolved waits. Live deltas ride a dedicated ephemeral channel, each stamped with the reply id of the durable event that will supersede it. Turn-cancel fires the session's retained cancel handle and relaunches the program over the retained event log - a stop reason, never an error - while `AgentSessions::close` ends a session for good.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
