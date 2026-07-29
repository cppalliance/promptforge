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

## What's next

Tranche 2: the `call` control-flow tool, the tool-call loop, and multi-section
fall-through (context clears on each transition). Then Lua. Gateway hardening
(admission control, pinning, packs, hot reload, Anthropic shim, streaming) is
deferred until self-hosted pods exist.

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
- No Lua yet (tranche 2). Streaming later (Talktron needs it).

## Open questions

- call syntax: positional vs table-form
- Context-preserving call: support it or not?
