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
```

The command provisions the pinned artifacts under repository-root `.model-cache/`, starts `llama-server` directly, runs the text and tool-call fixtures, and tears the server down. It never starts or routes through `promptforge-gateway`. `GatewayClient` connects directly to the selected loopback port at `/v1` with the successful attempt's private bearer token and unique model alias.

The process guard makes at most four fresh-port attempts. Each attempt asks the operating system for a free loopback port, creates a unique model alias and bearer token, starts the child with piped output, and drains each stream continuously into an independent 64 KiB tail buffer. Readiness requires both a successful `GET /health` and an authenticated `GET /v1/models` response containing that attempt's alias, with the child checked for exit before and after the probe. If the child exits while another listener remains on the selected port, the guard treats that as a bind collision and retries on a newly selected port. Other early exits fail immediately. Every attempt retains the 180-second readiness deadline, and failures include bounded stdout and stderr tails while redacting the bearer token. Dropping the guard kills and waits for the child, including normal success, returned error, panic unwinding, and Ctrl-C. Ctrl-C is installed around the complete suite; an atomic cancellation flag interrupts blocking startup polling, while dropping an active scenario immediately drops the guard.

The exact pinned server argument shape is:

```text
llama-server --model <cached-gguf> --alias <per-attempt-model-alias> --api-key <per-attempt-secret> --host 127.0.0.1 --port <selected-free-port> --ctx-size 4096 --n-predict 256 --parallel 1 --seed 424242 --temp 0 --jinja --reasoning off --reasoning-format deepseek
```

These are supported by official llama.cpp release `b10082`. Jinja supplies OpenAI tool-call rendering and parsing from the model's embedded template. Reasoning is disabled, and the OpenAI response parser removes any empty Qwen thinking envelope. One slot, a fixed seed, temperature zero, a 4096-token context, and a 256-token generation ceiling keep both scenarios bounded and deterministic.

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

Downloads are streamed to `.part`, synchronized, verified, and renamed only after the digest matches. A per-artifact file lock synchronizes threads and processes, and every waiter rechecks the cache after acquiring its lock. Cache paths are confined below `.model-cache/`; symlink and Windows reparse-point components are rejected. Server archives reject portable traversal spellings, NTFS alternate-data-stream names, links, and unsupported entry types before extracting into a confined staging directory. Install tree markers cover file contents and Unix permission modes, so lost executable permission is detected and repaired from the cached archive. Cache hits make no request. Stale partials and corrupt model files, archives, executables, extracted dependencies, or permissions are rejected and repaired. Unsupported operating-system and architecture pairs fail before any request.

The executable prints `downloading pinned artifact` on a cold cache and `cache hit` for each verified artifact on later runs. A successful second invocation therefore demonstrates cache-only provisioning without relying on elapsed time. Ordinary tests never call these production URLs.
