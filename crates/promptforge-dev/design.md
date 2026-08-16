# `promptforge-dev`: what this crate owns and refuses to own

## Executive summary

`promptforge-dev` is the author-facing interactive loop for editing a PromptForge prompt against a gateway that is already up. It reads one file, prepares live resolution inputs, executes H1 once and then the sections, dumps the store for inspection, and optionally watches for saves. Infrastructure - spawning `promptforge-gateway`, downloading GGUF weights, launching `llama-server`, choosing context size or thinking mode for a local profile - is out of scope. Authors set `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY` after starting the gateway themselves.

## Owns

1. **Argument surface.** `<prompt.md> [input] [--watch]` only. Hand-rolled argv parsing; no clap. Unknown flags (including the old `--context` / `--max-tokens` / `--no-think`) fail with usage text.
2. **Required gateway env.** Both URL and key must be present and non-empty before any prompt parse. Friendly hard-fail names the missing variable and reminds the author to start the gateway.
3. **Live catalog and tools.** Model catalog comes from `fetch_model_catalog`. Tool registry mirrors the CLI: `WebFetch` always, `WebSearch` when credentials are present. No pinned Qwen catalog.
4. **Per-invocation execution id.** Each run mints `dev-` plus 128 random bits, prints `run id: ...` on stderr before observer traffic, and reuses that id for parse and execution. Watch-mode reruns mint a new id.
5. **Store dump and traces.** Clear `<stem>.store/` once at run start. `MirrorStore` writes store files as they mutate; `TraceCapture` writes `.trace/turn-N-*.json` on each model turn. End-of-run reconcile syncs orphans without wiping `.trace/`.
6. **Watch debounce.** Parent-directory watcher filtered to the prompt file name, 300 ms quiet period, dump writes must not retrigger reruns.

## Refuses

1. **Starting a gateway or llama-server.** No `GatewayGuard`, `GatewayProfile`, `DevServerOptions`, or process spawn of those binaries.
2. **CLI knobs for model runtime.** Context, thinking, and max tokens belong on `models.need` / `models.only` in the prompt file.
3. **Live-gateway integration tests in this crate.** Offline unit tests only; the scenario harness stays in `promptforge-core-tests`.

## Module map

| File | Job |
|---|---|
| `main.rs` | Env gate, argv, exit codes, Ctrl-C |
| `run.rs` | One-shot parse and execute with live H1 resolution |
| `watch.rs` | Debounced rerun loop |
| `dump.rs` | Store dump and `.trace/` |
| `tools.rs` | Live registry + picker catalog |

*2026-08-08 - Cursor Grok 4.5*
