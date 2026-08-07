# `promptforge-core-tests`

This unpublished workspace binary crate owns complete, author-shaped PromptForge files that exercise the public `promptforge-core` parse, deterministic declaration-binding, and offline execution APIs. It complements the core crate's narrow inline grammar and lifecycle tests without creating a second parser or fixture discovery mechanism.

## Offline fixture suite

Run the ordinary offline fixture harness with:

```text
cargo test -p promptforge-core-tests
```

Every fixture is registered by name with an explicit `include_str!` in `src/suite.rs`, so adding a file without adding its expected public structure, error contract, or execution assertion cannot silently expand the suite. Valid fixtures assert parsed frontmatter, titles, shared Lua, section trees, and phase boundaries. Invalid fixtures assert the public error variant and a stable message fragment. Tool-free execution fixtures run shared declarations through public `bind_tool_declarations` with a deterministic no-tools resolver, then use the public parsed-`Prompt` compatibility input to `execute::run`. They assert exact Lua checkpoint sequences, scalar preamble early return, and store-backed fall-through across isolated sections using stable execution IDs and a mutex-backed observer. A concurrent regression partitions one shared recording by execution ID.

The shipped-prompt smoke test discovers every Markdown file under the repository's `prompts/` directory, rejects concrete tool names, and requires each file to parse. The MCP server's owner tests retain the shipped-prompt semantic binding assertion against its complete live registry.

Ordinary tests do not load a generation model, start `llama-server`, call an external host, or require credentials. Artifact synchronization regressions use loopback fake HTTP servers, temporary caches, tiny generated archives, and copies of the test executable. A cold workspace build may still acquire the core tool picker's separately pinned build-time embedding assets.

## Explicit real-model suite

Run the opt-in real-model executable from the repository root:

```text
cargo run -p promptforge-core-tests
cargo run -p promptforge-core-tests -- scenarios
```

Both spellings run the same fixed scenario suite; the bare invocation defaults to `scenarios`. Any other argument shape fails with concise usage text and a failure exit code. The command provisions the pinned artifacts under repository-root `.model-cache/`, starts `llama-server` directly, runs the text and tool-call fixtures, and tears the server down. It never starts or routes through `promptforge-gateway`. `GatewayClient` connects directly to the selected loopback port at `/v1` with the successful attempt's private bearer token and unique model alias.

The process guard makes at most four fresh-port attempts. Each attempt asks the operating system for a free loopback port, creates a unique model alias and bearer token, starts the child with piped output, and drains each stream continuously into an independent 64 KiB tail buffer. Readiness requires both a successful `GET /health` and an authenticated `GET /v1/models` response containing that attempt's alias, with the child checked for exit before and after the probe. If the child exits while another listener remains on the selected port, the guard treats that as a bind collision and retries on a newly selected port. Other early exits fail immediately. Every attempt retains the 180-second readiness deadline, and failures include bounded stdout and stderr tails while redacting the bearer token. Dropping the guard kills and waits for the child, including normal success, returned error, panic unwinding, and Ctrl-C. Ctrl-C is installed around the complete suite; an atomic cancellation flag interrupts blocking startup polling, while dropping an active scenario immediately drops the guard.

The process guard selects its flags through a server profile. The explicit suite always launches the scenario profile, whose exact pinned argument shape is:

```text
llama-server --model <cached-gguf> --alias <per-attempt-model-alias> --api-key <per-attempt-secret> --host 127.0.0.1 --port <selected-free-port> --ctx-size 4096 --n-predict 256 --parallel 1 --seed 424242 --temp 0 --jinja --reasoning off --reasoning-format deepseek
```

These are supported by official llama.cpp release `b10082`. Jinja supplies OpenAI tool-call rendering and parsing from the model's embedded template. Reasoning is disabled, and the OpenAI response parser removes any empty Qwen thinking envelope. One slot, a fixed seed, temperature zero, a 4096-token context, and a 256-token generation ceiling keep both scenarios bounded and deterministic.

The guard also defines a dev profile for interactive prompt development: a configurable context (`--ctx-size`, default 131072) and generation ceiling (`--n-predict`, default 8192), `--flash-attn on`, a q8_0-quantized KV cache, full GPU offload (`-ngl 99`), `--jinja`, `--reasoning-format auto` with thinking enabled and model-card sampling (`--temp 1.0 --top-p 0.95 --top-k 20 --presence-penalty 1.5`), and `--parallel 1`. A no-think variant turns reasoning off and switches sampling to the non-thinking preset (`--temp 0.7 --top-p 0.8`). The `dev` subcommand launches this profile.

## Dev command

Run one prompt file against the dev profile from the repository root:

```text
cargo run -p promptforge-core-tests -- dev <prompt-file> [input] [--watch] [--context N] [--max-tokens N] [--no-think]
```

The positional `input` defaults to empty. `--context` sets `--ctx-size` and `--max-tokens` sets `--n-predict`; both take a positive integer and fall back to the profile defaults above. `--no-think` selects the non-thinking preset. Flags and positionals may interleave; unknown arguments, a missing prompt file argument, and malformed numbers fail with the usage text before any provisioning.

The dev path provisions the dev-kind artifacts on a blocking task, starts the guarded `llama-server` with the dev profile built from the flags, and runs the prompt through `src/dev.rs`. Single-shot mode prints the result to stdout and exits; a failure prints the error beside the server's bounded diagnostics to stderr and exits nonzero.

`--watch` keeps the provisioned artifacts and the guarded server warm and reruns the prompt after every save. Because editors often save through an atomic rename, the watcher (workspace `notify`) covers the prompt file's parent directory filtered to the prompt's file name, and a 300 ms quiet period coalesces each burst of change notifications into one rerun. Every rerun re-reads, re-parses, re-binds, and re-executes the file; results print to stdout while watch status lines stay on stderr. A failed read, parse, bind, or run prints the error with the server diagnostics and keeps watching. Ctrl-C is handled by the same `tokio::select!` wrapper as every other mode and tears the server down through the guard.

Watch-mode logic is tested offline: the debounce and rerun loop is factored behind a channel seam, so tests inject fake change events and observe rerun requests without any filesystem watcher, server, or download.

`prompts/dev/` holds the dev-loop verification prompts, which are not part of the offline fixture suite: `dev-verification.md` exercises one substituted model reply plus an epilog against the real dev model, and `dev-search-absent.md` declares a web-search capability so an environment without `PROMPTFORGE_TOKEN` demonstrates the loud `Absent` failure at bind, before any model call.

The dev path is verified end to end on Windows x86-64 against the Vulkan build: the cold run downloads the model and the GPU server archive, and an immediate rerun reports only cache hits before passing. The pinned model loads and answers at the default 131072-token context on a 24 GB GPU (RTX 4090); no smaller `--context` fallback was needed on that hardware. Interrupting `--watch` with Ctrl-C tears the guarded `llama-server` down through the guard, leaving no orphan process. Note that Ctrl-C delivery is a console event on Windows: a parent that spawns this binary with `CREATE_NEW_PROCESS_GROUP` (Cygwin and MSYS shells do) suppresses it, and force-terminating the process bypasses guard teardown entirely, as `TerminateProcess` runs no destructor.

## Dev runner module

`src/dev.rs` holds the single-shot dev-mode runner. Its crate-internal entry, `run_once(prompt_path, input, base_url, api_key, model_alias)`, mirrors the CLI pipeline: it reads the file, refuses any file whose frontmatter declares no `promptforge:` version, parses, builds the live tool registry and a picker catalog derived from the same instances, binds, and executes with `GatewayClient` pointed at the caller's already-running guarded server - never through `promptforge-gateway`. Each run generates its own `dev-{nonce:016x}` execution ID. A verbose observer writes every `(execution, section, detail)` record - including `Lua: ...` checkpoints - as one line to stderr, so stdout stays free for the caller to print the returned result string alone. On failure the runner returns the error rather than printing it, leaving the caller to render it beside the bounded server diagnostics it owns.

The registry ports the CLI's construction: local `web_fetch` is always live; gateway-backed `web_search` joins only when `PROMPTFORGE_TOKEN` is present, with `PROMPTFORGE_BASE_URL` naming the gateway and the CLI's default URL assumed when unset. A prompt needing search without a gateway credential fails loudly as `Absent` at bind. Environment reading is factored behind an injectable lookup, so offline tests compose the registry both ways without mutating the process environment. Empty server arguments, unreadable paths, and non-promptforge files are rejected with clear errors before any network work. The `dev` subcommand reaches this module for single-shot runs and through the watch loop; its offline tests never start a server, download an artifact, or touch the network.

`execution/real-text.md` requires one nonempty text completion carrying a requested marker. Its Lua epilog prefixes the returned result, so the runner proves both reply binding and epilog visibility, and the observer must report exactly one completed model turn.

`execution/real-tool-call.md` exposes one concrete one-string tool under the prompt-local alias `ask_fixture`, distinct from its concrete wire name. The fixture requires a call with exactly `{"value":"promptforge-probe"}`. The tool rejects any extra, missing, or non-string argument, returns a deterministic result unavailable before dispatch, and records the parsed arguments. The runner proves one schema-valid aliased dispatch, a tool-result continuation, a nonempty final answer carrying both final and result markers, epilog visibility, exactly one successful tool call, and exactly two completed model turns under the fixture's two-turn budget. Its one-entry picker uses a zero similarity floor and margin so the scenario tests binding mechanics without making semantic ranking another live-model variable.

## Pinned real-model artifacts

The provisioner caches all files under repository-root `.model-cache/`, which is gitignored:

- Windows x86-64: `https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-win-cpu-x64.zip`, SHA-256 `d606bd97164b61a3f504ded91d5c9a19f94281c6ac2e4672e09f85f41a232076`.
- Windows arm64: `https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-win-cpu-arm64.zip`, SHA-256 `50dab63396f579cc0ceb4a4fc4b985414d55aaebd4722f363ad03696648711a4`.
- Ubuntu x86-64: `https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-ubuntu-x64.tar.gz`, SHA-256 `01dcc9257ea1030bed5034aae667cd38c7f9cb620fd3e06c303d3813dd9e7d95`.
- Ubuntu arm64: `https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-ubuntu-arm64.tar.gz`, SHA-256 `16baaea628e228d0c546f4ddc9bef1b5182201caca75f65baa5e73ddff8d1204`.
- macOS x86-64: `https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-macos-x64.tar.gz`, SHA-256 `5a28fad0f05bf283c1adb92224c1bf3c25ee06acd0f4065b170016c14b490473`.
- macOS arm64: `https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-macos-arm64.tar.gz`, SHA-256 `d644e16eefef3402e4fa86c0fcdce3b00a6786db68c3f216875ce87b45d29173`.
- Official `Qwen/Qwen3-0.6B-GGUF` file `Qwen3-0.6B-Q8_0.gguf`, approximately 639 MB, from `https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/main/Qwen3-0.6B-Q8_0.gguf?download=true`, SHA-256 `9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031`.

Provisioning is parameterized by model kind. The entries above form the scenario kind, which the explicit suite always provisions. The dev kind pins its own model and GPU-enabled server archives from the same `b10082` release, installed under distinct platform keys (for example `windows-x86_64-vulkan`) so CPU and GPU installs coexist in `.model-cache/llama.cpp/`. One kind never downloads the other kind's artifacts. The `dev` subcommand provisions the dev kind; the scenario suite provisions only the scenario kind. Dev archive digests are read from the GitHub release API (`https://api.github.com/repos/ggml-org/llama.cpp/releases/tags/b10082`, per-asset `digest` field):

- Windows x86-64 dev (Vulkan): `https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-win-vulkan-x64.zip`, SHA-256 `0a4b2e41cfb950da9a749baf8978e0626690fbead3b0ca96860785484cda5bde`.
- Ubuntu x86-64 dev (Vulkan): `https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-ubuntu-vulkan-x64.tar.gz`, SHA-256 `9003ea32e3d5d8a01da3e4b5d3124e0d21c63d51e112c40f5dcdef91ffaca7cc`.
- Ubuntu arm64 dev (Vulkan): `https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-ubuntu-vulkan-arm64.tar.gz`, SHA-256 `2805902c3074f615a0105a5325ee29799500c8e29c90ccb986b59e1141df551e`.
- macOS dev reuses the Metal-enabled macOS tar entries above; Windows arm64 dev has no Vulkan build in `b10082` and falls back to the CPU zip above.
- Community `unsloth/Qwen3.5-9B-GGUF` file `Qwen3.5-9B-Q4_K_M.gguf`, 5,680,522,464 bytes, from `https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q4_K_M.gguf`, SHA-256 `03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8` (verified against the Hugging Face LFS metadata).

Downloads are streamed to `.part`, synchronized, verified, and renamed only after the digest matches. A per-artifact file lock synchronizes threads and processes, and every waiter rechecks the cache after acquiring its lock. Cache paths are confined below `.model-cache/`; symlink and Windows reparse-point components are rejected. Server archives reject portable traversal spellings, NTFS alternate-data-stream names, links, and unsupported entry types before extracting into a confined staging directory. Install tree markers cover file contents and Unix permission modes, so lost executable permission is detected and repaired from the cached archive. Cache hits make no request. Stale partials and corrupt model files, archives, executables, extracted dependencies, or permissions are rejected and repaired. Unsupported operating-system and architecture pairs fail before any request.

The executable prints `downloading pinned artifact` on a cold cache and `cache hit` for each verified artifact on later runs. A successful second invocation therefore demonstrates cache-only provisioning without relying on elapsed time. These per-artifact status lines follow the mode's status stream: stdout for the scenario suite, whose output shape is pinned, and stderr for dev mode, whose stdout carries only the final result. Ordinary tests never call these production URLs.
