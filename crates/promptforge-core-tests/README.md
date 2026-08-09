# `promptforge-core-tests`

This unpublished workspace binary crate owns complete, author-shaped PromptForge files that exercise the public `promptforge-core` parse, deterministic declaration-binding, and offline execution APIs, plus an opt-in 0.6B scenario harness that self-hosts a temporary gateway for CI. It complements the core crate's narrow inline grammar and lifecycle tests without creating a second parser or fixture discovery mechanism.

Interactive prompt development lives in `promptforge-dev`, not here.

## Offline fixture suite

Run the ordinary offline fixture harness with:

```text
cargo test -p promptforge-core-tests
```

Every fixture is registered by name with an explicit `include_str!` in `src/suite.rs`, so adding a file without adding its expected public structure, error contract, or execution assertion cannot silently expand the suite. Valid fixtures assert parsed frontmatter, titles, shared Lua, section trees, and phase boundaries. Invalid fixtures assert the public error variant and a stable message fragment. Tool-free execution fixtures parse and run directly through the public live H1 path with an empty picker and model catalog. They assert exact Lua checkpoint sequences, scalar prologue early return, and store-backed fall-through across isolated sections using stable execution IDs and a mutex-backed observer. A concurrent regression partitions one shared recording by execution ID.

The shipped-prompt smoke test discovers every Markdown file under the repository's `prompts/` directory, rejects concrete tool names, and requires each file to parse. The MCP server's owner tests retain the shipped-prompt semantic binding assertion against its complete live registry.

Ordinary tests do not load a generation model, start `promptforge-gateway`, call an external host, or require credentials. A cold workspace build may still acquire the core tool picker's separately pinned build-time embedding assets.

## Explicit 0.6B scenario suite

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

`execution/real-text.md` requires one nonempty text completion carrying a requested marker. Its Lua epilog prefixes the returned result, so the runner proves both reply binding and epilog visibility, and the observer must report exactly one completed model turn.

`execution/real-tool-call.md` exposes one concrete one-string tool under the prompt-local alias `ask_fixture`, distinct from its concrete wire name. The fixture requires a call with exactly `{"value":"promptforge-probe"}`. The tool rejects any extra, missing, or non-string argument, returns a deterministic result unavailable before dispatch, and records the parsed arguments. The runner proves one schema-valid aliased dispatch, a tool-result continuation, a nonempty final answer carrying both final and result markers, epilog visibility, exactly one successful tool call, and exactly two completed model turns under the fixture's two-turn budget. Its one-entry picker uses a zero similarity floor and margin so the scenario tests binding mechanics without making semantic ranking another live-model variable.

## Pinned model (via gateway profile)

Model URL and sha256 pins are written into the generated profile and match the constants in `promptforge-gateway`'s local artifact module:

- Scenario: official `Qwen/Qwen3-0.6B-GGUF` file `Qwen3-0.6B-Q8_0.gguf`, approximately 639 MB, SHA-256 `9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031`.

The gateway caches downloads under `~/.promptforge/` (not repository-root `.model-cache/`). Ordinary unit tests never call these production URLs.
