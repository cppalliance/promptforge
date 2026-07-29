# PromptForge Status

## What exists and works

Gateway v0 complete. Two-process vertical slice: `promptforge run` -> the gateway
(which holds the vendor key) -> the backend -> "Hello, world!". The executor no
longer holds any vendor credential, only the gateway URL and shared token.

- `promptforge-core::parser` - frontmatter + H1/description + recursive H2-H6 tree + leading Lua-fence separation. 10 unit tests.
- `promptforge-core::client::GatewayClient` - OpenAI-shaped client pointed at the gateway (URL + shared token; no vendor key).
- `promptforge-core::execute::run` - one round trip on the entry section.
- `promptforge-cli` - `promptforge run <file.md>`.
- `promptforge-gateway` - axum service: `gateway.toml` (Secret + ${VAR} interpolation), model routing, one OpenAI passthrough upstream, bearer auth, POST /v1/chat/completions, GET /health. Config + routing unit tests + 4 end-to-end tests (fake backend + real client).

Lua commit 1 (echo): mlua embedded + sandboxed; `args` (single raw string) exposed
to a section's Lua block; the finish case of the exit rule works - a chunk that
returns a plain value ends the run with it. `promptforge run prompts/echo.md "x"`
runs `return args` and prints `x` with no model call and no gateway.

- `promptforge-core::lua::run_chunk` - sandboxed VM (string/table/math + base only; no io/os/require/load/debug), instruction-count hook, args in, top-level return out. 5 unit tests.
- `execute::run(prompt, args)` - Lua chunk returns a value -> finish; else send prose to the gateway. Client built lazily so a Lua-only run needs no credentials.
- CLI: `promptforge run <file> [input]` passes `input` as `args`.

## What's next

Lua commit 2: `{{ args }}` / `{{ var.x }}` substitution + the writable `var`
table, wired before the model turn (the greet.md demo). Then the control-flow
tranche: the rest of the exit rule (nil = fall-through, goto/task/fanout
descriptors), the tool-call loop, and multi-section flow. Gateway hardening
(admission, pinning, packs, hot reload, Anthropic shim, streaming) deferred until
self-hosted pods exist.

## How to run

Two processes. The gateway holds the credentials; the client points at it.

```
export ANTHROPIC_API_KEY=sk-ant-...      # the vendor key, only the gateway sees it
export PROMPTFORGE_TOKEN=dev-secret      # shared bearer, both processes
cargo run -p promptforge-gateway -- serve gateway.toml &

export PROMPTFORGE_BASE_URL=http://127.0.0.1:8081/v1
cargo run -p promptforge-cli -- run prompts/hello.md
```

## Decisions settled

- Rust multi-crate workspace: promptforge-core (lib), promptforge-cli (bin)
- Edition 2024, resolver 3, rust-version 1.85
- Lint policy in [workspace.lints]: unsafe_code forbid, missing_docs, clippy all=deny + pedantic=warn, unwrap_used deny (doc_markdown allowed for product names); tests allowed unwrap/expect via clippy.toml
- Public error types are #[non_exhaustive] and do not leak dependency error types
- Gateway is the only process with an edge to a backend; it holds vendor keys
- Executor targets the gateway: PROMPTFORGE_BASE_URL (default local gateway) + PROMPTFORGE_TOKEN (required)
- Wire structs are NOT shared between core and gateway (JSON is the contract; each side owns its view)
- Gateway v0 supports only protocol = "openai"; Anthropic shim deferred
- Default model claude-sonnet-4-6; override with PROMPTFORGE_MODEL
- Entry point is the first H2, not a named section
- Recursive heading nesting (H2-H6); skipped levels tolerated
- Section ends when the model returns text with no tool calls (auto termination)
- tool_choice: auto when tools present, required when only call is present, omitted when no tools
- `call` is the unified control-flow tool (type discriminator: return, goto, task, fanout) - keeps control-flow tool count at 1
- Lua via mlua (lua54, vendored); sandbox = string/table/math + safe base, no io/os/require/load/debug, instruction-budget hook
- args is a single raw string (caller input); derived values are deduced+stored by the pipeline, not passed
- Exit rule (finish case only so far): a Lua chunk that returns a plain value ends the run with it; no return / no Lua -> model path
- Streaming later (Talktron needs it)

## Open questions

- call syntax: positional vs table-form
- Context-preserving call: support it or not?
