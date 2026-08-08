# `promptforge-core-tests`

This unpublished workspace binary crate owns complete, author-shaped PromptForge files that exercise the public `promptforge-core` parse, deterministic declaration-binding, and offline execution APIs. It complements the core crate's narrow inline grammar and lifecycle tests without creating a second parser or fixture discovery mechanism.

## Offline fixture suite

Run the ordinary offline fixture harness with:

```text
cargo test -p promptforge-core-tests
```

Every fixture is registered by name with an explicit `include_str!` in `src/suite.rs`, so adding a file without adding its expected public structure, error contract, or execution assertion cannot silently expand the suite. Valid fixtures assert parsed frontmatter, titles, shared Lua, section trees, and phase boundaries. Invalid fixtures assert the public error variant and a stable message fragment. Tool-free execution fixtures run shared declarations through public `bind_tool_declarations` with a deterministic no-tools resolver, then use the public parsed-`Prompt` compatibility input to `execute::run`. They assert exact Lua checkpoint sequences, scalar preamble early return, and store-backed fall-through across isolated sections using stable execution IDs and a mutex-backed observer. A concurrent regression partitions one shared recording by execution ID.

The shipped-prompt smoke test discovers every Markdown file under the repository's `prompts/` directory, rejects concrete tool names, and requires each file to parse. The MCP server's owner tests retain the shipped-prompt semantic binding assertion against its complete live registry.

Ordinary tests do not load a generation model, start `promptforge-gateway`, call an external host, or require credentials. A cold workspace build may still acquire the core tool picker's separately pinned build-time embedding assets.

## Explicit real-model suite

Build the gateway once, then run the opt-in real-model executable from the repository root:

```text
cargo build -p promptforge-gateway
cargo run -p promptforge-core-tests
cargo run -p promptforge-core-tests -- scenarios
```

Both spellings run the same fixed scenario suite; the bare invocation defaults to `scenarios`. Any other argument shape fails with concise usage text and a failure exit code.

The harness writes a temporary gateway profile TOML that declares the pinned scenario model (`Qwen3-0.6B` URL + sha256), starts `promptforge-gateway serve <temp.toml>` on a free loopback port with a random bearer token, waits until `GET /health` and authenticated `GET /v1/models` advertise the local model, then runs the text and tool-call fixtures through `GatewayClient` pointed at that gateway. The gateway owns GGUF download/caching under `~/.promptforge/` and spawns `llama-server`. Dropping the guard kills the gateway process tree (including child `llama-server` processes).

Override the gateway binary with `PROMPTFORGE_GATEWAY_BIN` when needed. Otherwise the harness looks for `target/debug/promptforge-gateway` then `target/release/promptforge-gateway` (with `.exe` on Windows) relative to the workspace.

The generated scenario profile uses CPU-oriented knobs: `context = 4096`, `n_predict = 256`, `thinking = "never"`, `gpu_layers = 0`, `flash_attention = false`. Exact historical `llama-server` flags (fixed seed, temperature zero, `reasoning-format deepseek`) are no longer set by core-tests; those details live inside the gateway's local launch path.

Ctrl-C is installed around the complete suite; an atomic cancellation flag interrupts blocking startup polling, while dropping an active scenario immediately drops the guard.

## Dev command

Run one prompt file against the generated dev profile from the repository root:

```text
cargo build -p promptforge-gateway
cargo run -p promptforge-core-tests -- dev <prompt-file> [input] [--watch] [--context N] [--max-tokens N] [--no-think]
```

The positional `input` defaults to empty. `--context` sets `[[local_model]].context` and `--max-tokens` sets `[[local_model]].n_predict`; both take a positive integer and fall back to the profile defaults (131072 and 8192). `--no-think` writes `thinking = "never"`; otherwise `thinking = "switchable"`. Flags and positionals may interleave; unknown arguments, a missing prompt file argument, and malformed numbers fail with the usage text before any launch.

The dev path generates a profile for the pinned `Qwen3.5-9B` model with GPU-oriented knobs (`gpu_layers = 99`, flash attention on, q8_0 KV cache), starts the guarded gateway, and runs the prompt through `src/dev.rs`. Single-shot mode prints the result to stdout and exits; a failure prints the error beside the gateway's bounded diagnostics to stderr and exits nonzero.

After every executed run, success or failure, the run's store is dumped to a `<prompt-stem>.store` directory beside the prompt file (for `briefer.md` that is `briefer.store/`), so a `store.write("evidence.md", reply)` lands at `briefer.store/evidence.md` for inspection. The directory is cleared before each dump, removed entirely when the store is empty, and each dumped path is announced on stderr; a store path that cannot map to a safe relative filesystem path is reported on stderr and skipped. Raw model turns are written under `<prompt-stem>.store/.trace/` as `turn-N-request.json` and `turn-N-response.json` after that dump (announced on stderr the same way), so a cleared store directory never erases the turn files from the run just finished. Watch mode dumps after every rerun, and the watcher's file-name filter keeps the dump writes from retriggering reruns.

`--watch` keeps the guarded gateway warm and reruns the prompt after every save. Because editors often save through an atomic rename, the watcher (workspace `notify`) covers the prompt file's parent directory filtered to the prompt's file name, and a 300 ms quiet period coalesces each burst of change notifications into one rerun. Every rerun re-reads, re-parses, re-binds, and re-executes the file; results print to stdout while watch status lines stay on stderr. A failed read, parse, bind, or run prints the error with the gateway diagnostics and keeps watching. Ctrl-C is handled by the same `tokio::select!` wrapper as every other mode and tears the gateway down through the guard.

Watch-mode logic is tested offline: the debounce and rerun loop is factored behind a channel seam, so tests inject fake change events and observe rerun requests without any filesystem watcher, gateway, or download.

`prompts/dev/` holds the dev-loop verification prompts, which are not part of the offline fixture suite: `dev-verification.md` exercises one substituted model reply plus an epilog against the real dev model, and `dev-search-absent.md` declares a web-search capability so an environment without `PROMPTFORGE_GATEWAY_KEY` demonstrates the loud `Absent` failure at bind, before any model call.

## Dev runner module

`src/dev.rs` holds the single-shot dev-mode runner. Its crate-internal entry, `run_once(prompt_path, input, base_url, api_key, model_alias)`, mirrors the CLI pipeline: it reads the file, refuses any file whose frontmatter declares no `promptforge:` version, parses, builds the live tool registry and a picker catalog derived from the same instances, binds against `pinned_qwen_dev_catalog` (one switchable entry for the pinned Qwen3.5 alias so H1 `models.need` can resolve), and executes with `GatewayClient` pointed at the caller's already-running gateway. Each run generates its own `dev-{nonce:016x}` execution ID. A verbose observer writes every `(execution, section, detail)` record - including `Lua: ...` checkpoints and the payload-free `Model turn truncated` detail - as one line to stderr, so stdout stays free for the caller to print the returned result string alone. Empty model product fails the run as `EmptyModelReply` / `Model turn failed` rather than a soft empty-reply observation. An always-on `DebugCapture` writes raw turn JSON under `<prompt-stem>.store/.trace/` (see the dump paragraph above). On failure the runner returns the error rather than printing it, leaving the caller to render it beside the bounded gateway diagnostics it owns.

The registry ports the CLI's construction: local `web_fetch` is always live; gateway-backed `web_search` joins only when both `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_KEY` are present. A prompt needing search without both fails loudly as `Absent` at bind. Environment reading is factored behind an injectable lookup, so offline tests compose the registry both ways without mutating the process environment. Empty server arguments, unreadable paths, and non-promptforge files are rejected with clear errors before any network work. The `dev` subcommand reaches this module for single-shot runs and through the watch loop; its offline tests never start a gateway, download an artifact, or touch the network.

`execution/real-text.md` requires one nonempty text completion carrying a requested marker. Its Lua epilog prefixes the returned result, so the runner proves both reply binding and epilog visibility, and the observer must report exactly one completed model turn.

`execution/real-tool-call.md` exposes one concrete one-string tool under the prompt-local alias `ask_fixture`, distinct from its concrete wire name. The fixture requires a call with exactly `{"value":"promptforge-probe"}`. The tool rejects any extra, missing, or non-string argument, returns a deterministic result unavailable before dispatch, and records the parsed arguments. The runner proves one schema-valid aliased dispatch, a tool-result continuation, a nonempty final answer carrying both final and result markers, epilog visibility, exactly one successful tool call, and exactly two completed model turns under the fixture's two-turn budget. Its one-entry picker uses a zero similarity floor and margin so the scenario tests binding mechanics without making semantic ranking another live-model variable.

## Pinned models (via gateway profile)

Model URL and sha256 pins are written into the generated profile and match the constants in `promptforge-gateway`'s local artifact module:

- Scenario: official `Qwen/Qwen3-0.6B-GGUF` file `Qwen3-0.6B-Q8_0.gguf`, approximately 639 MB, SHA-256 `9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031`.
- Dev: community `unsloth/Qwen3.5-9B-GGUF` file `Qwen3.5-9B-Q4_K_M.gguf`, SHA-256 `03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8`.

The gateway caches downloads under `~/.promptforge/` (not repository-root `.model-cache/`). Ordinary unit tests never call these production URLs.
