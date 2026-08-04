# PromptForge Status

## What exists and works

Six crates, three binaries. The gateway holds the vendor credential; both other
binaries reach a model only through it. The MCP server is complete and serving:
`promptforge-mcp-server serve [--stdio] prompts.toml` runs one of this
repository's own prompts that Cursor or Claude Code names to `run_prompt`.

- `promptforge-core` (lib) - the runtime. `parser` (frontmatter, H1 + description, recursive H2-H6 tree, leading Lua fence); `client::GatewayClient` (OpenAI-shaped, aimed at the gateway, public `DEFAULT_MODEL`); `lua::run_chunk` (mlua sandbox: string/table/math + safe base, no io/os/require/load/debug, instruction-budget hook); `subst::substitute` (`{{ args }}`, `{{ var.x }}`, `{{ sys.x }}`; pure lookup, no arithmetic - compute in Lua); `store` (run-scoped VFS, memory or files, exposed to Lua); `observe`, the progress seam (`Observer`, non_exhaustive `Event` = `RunStarted`/`SectionStarted`/`SectionFinished`/`ModelTurn`/`ToolCalled`/`RunFinished`, `NullObserver`); and `execute::run(prompt, args, tools, store, opts)`, which walks top-level sections in file order, each in a fresh context - a Lua `return` finishes the run with no model call, otherwise the prose is substituted, takes one round trip, and control falls through. Off the end yields `default_return`, else the last reply, else a generic completion. `opts` is `RunOptions { observer, client }`; `client: None` builds from the environment (the CLI's path) and `Some(..)` is how a file-configured caller supplies one, since setting an env var is unsafe under edition 2024. Every section boundary and tool call is reported through the observer, and dropping an event cannot change a result. `design-core.md` is its design document and is current; everything designed and unbuilt stays in the design repository as `design-core-residue.md`. 97 unit + 23 doc tests.
- `promptforge-webfetch` (lib) - the `web_fetch` tool: an in-process GET, a readability pass to the article body, and a whole-page markdown fallback, behind a URL and address policy that blocks private ranges and checks redirects. No credential, so it runs wherever the prompt runs. 48 unit tests.
- `promptforge-cli` (bin `promptforge`) - `promptforge run <file.md> [input]`; refuses a file that declares no `promptforge:` version. `design-cli.md` is its design document and is current. 5 unit tests.
- `promptforge-gateway` (bin) - axum service: `gateway.toml` (`Secret` + `${VAR}` interpolation), model routing, one OpenAI passthrough upstream, bearer auth, `POST /v1/chat/completions`, `POST /v1/tools/web_search` (Brave), `GET /health`. `design-gateway.md` is its design document and is current; everything designed and unbuilt stays in the design repository as `design-gateway-residue.md`. 12 unit + 7 integration tests.
- `promptforge-mcp-server` (bin) - the MCP server, complete. `config` parses `prompts.toml` (`Secret`, `${VAR}`, every table `deny_unknown_fields`; only `[server]` and `[gateway]` required; `[server].token` is optional because only HTTP reads it - `serve` refuses to bind without one and stdio boots without one - and a present-but-blank token is refused at load; interpolation runs over the parsed document, so an unset variable is attributed to the field that carried it). `catalog` is one resolution pass for both boot and reload: expand `[catalog].include`, subtract `exclude` (both relative to `[paths].prompts`, `*` stopping at a separator), then apply the `[prompts.NAME]` exceptions. A file declaring no `promptforge:` version is not a prompt and is skipped silently; anything else must read, parse, and declare a name matching `^[a-z][a-z0-9_]{0,47}$` that is not one of the four built-in names. Faults accumulate and print together. `OnBroken` is the only boot/reload difference: boot rejects, a reload retains the prompt as a broken entry carrying its error. `tools` is `tools/list`, and it is the same four built-ins for every catalog - `list_prompts`, `run_prompt`, `check_run`, and `need_prompt` when the `picker` feature is compiled in. No prompt is published as a tool of its own: a prompt is a command, run because a caller named it to `run_prompt`, so nothing here competes for a model's tool selection and no client's cached tool list can go stale. Publication is read from one table, so the dispatcher can never answer a tool `tools/list` does not offer. `server` runs a call and reports a `RunResult` (`run_id`, `prompt`, `version`, `status`, `value`, `turns`, `elapsed_ms`, `error`) in `structuredContent` with the value verbatim beside it; a name is resolved case-folded with `-` as `_` and never fuzzily, a miss listing every enabled name closest first. `progress` forwards events as `notifications/progress` over a bounded queue written with `try_send`, so a slow reader loses frames rather than slowing the run; it also logs the two run boundaries at `info` (the prompt, then the turn count, elapsed time, and outcome), a failed tool call at `warn`, and everything inside a run at `debug`, so the default level shows that a run happened and how long it took. `registry` admits `max_concurrent_runs` at a time, converts a call past `reply_deadline` into a `running` result carrying a `run_id`, and keeps a finished record collectable for `retain_completed`; it logs at `info` both the run that outlived its call (by the same id the caller was given) and that run's later outcome, an admission refusal at `warn`, and eviction at `debug`. `transport` nests streamable HTTP at `/mcp` behind a constant-time bearer check with `/healthz` registered outside the layer, or serves stdio, where nothing is bound and no token is read or needed. `watch` re-resolves on save and swaps an `ArcSwap` a run in flight is unaffected by; nothing is announced to a client, because the tool list never moves and every call reads the catalog fresh, so a prompt saved mid-session is callable at once with no reconnect. `retrieval` answers `need_prompt` from an index over every runnable prompt's name and description, rebuilt on the same swap only when one of those moved. `rmcp` is pinned at `=3.1.0`; `--no-default-features` drops the picker's 67MB of weights and `need_prompt` with them. 148 unit + 6 binary + 8 integration + 5 doc tests.
- `promptforge-tool-picker` (lib) - a pure, deterministic, embedding-based engine that resolves a plain-English capability against an abstract tool catalog. `build_with(Arc<Embedder>, Catalog, Config)` is the one indexing path, `build` wraps it, and `rebuild(catalog)` re-indexes over the loaded weights, which is what the MCP server's save-time rebuild rides. bge-small-en-v1.5 is compiled into the library by `build.rs` (pinned commit, hardcoded SHA-256, downcast to fp16, embedded from `OUT_DIR`); the first build anywhere needs Hugging Face access, later ones use its cache. `embed` returns a 384-dimension CLS-pooled unit vector; `resolve` answers `Bind`, `Duplicate`, `Ambiguous`, or `Absent` under `similarity_floor` 0.825, `duplicate_threshold` 0.98, and `margin` 0.05, and `shortlist(need, k)` reports the same ranking without judging it. Abstention is an outcome, never an error. `design-tool-picker.md` is its design document and is current. 99 unit + 12 integration + 7 doc tests.

Prompts in `prompts/`: `echo.md` (`return args`, no gateway), `greet.md` (Lua-computed
`var` through the gateway), `hello.md`, `research_person` in `research-person.md`
(`web_search` + `web_fetch`). `gateway.toml` and `prompts.toml` at the root are
both working development profiles.

## What's next

Generate `design-mcp-server.md` from the finished MCP server. Deferred: live time across
turns (`sys.live("now")` tail refresh, or a `now()` tool); the `facts` bag; the
control-flow tranche - the rest of the exit rule (goto/task/fanout descriptors)
and durable state to carry a non-terminal section's work forward. Gateway
hardening (admission, pinning, packs, hot reload, Anthropic shim, streaming) waits
on self-hosted pods.

## How to run

```
export ANTHROPIC_API_KEY=sk-ant-...      # the vendor key, only the gateway sees it
export PROMPTFORGE_TOKEN=dev-secret      # shared bearer, every process
cargo run -p promptforge-gateway -- serve gateway.toml &

export PROMPTFORGE_BASE_URL=http://127.0.0.1:8081/v1
cargo run -p promptforge-cli -- run prompts/hello.md

export PROMPTFORGE_MCP_TOKEN=dev-secret  # the harness presents this to /mcp
cargo run -p promptforge-mcp-server -- serve prompts.toml

cargo run -p promptforge-mcp-server -- serve --stdio prompts.toml   # no MCP token needed
```

Run the MCP server from the repository root: `prompts.toml` names its paths
relative to the working directory. README.md carries the Cursor and Claude Code
configuration and the developer loop (write a prompt, save it, call it - the
published tool list never changes, so no client restart is ever needed).

## Decisions settled

- Rust workspace, edition 2024, resolver 3, rust-version 1.89 (floor set by candle and hf-hub)
- Lint policy in `[workspace.lints]`: unsafe_code forbid, missing_docs, clippy all=deny + pedantic=warn, unwrap_used and expect_used deny (doc_markdown allowed for product names); tests allow unwrap/expect via clippy.toml
- Public error types are `#[non_exhaustive]` and leak no dependency's error type; tool-picker's `Error` keeps matchable variants plus a `detail` string, since two of them carry no cause
- Integration tests are one binary per crate at `tests/it/main.rs`, an area per module
- The gateway is the only process with an edge to a backend; it holds vendor keys
- Wire structs are not shared between core and gateway (JSON is the contract)
- Gateway v0 speaks `protocol = "openai"` only; Anthropic shim deferred
- Default model claude-sonnet-4-6; override with `PROMPTFORGE_MODEL` or `[gateway].model`
- Entry point is the first H2; recursive H2-H6 nesting, skipped levels tolerated
- A section ends when the model returns text with no tool calls; 24 round trips per section by default
- `call` is the one control-flow tool (type discriminator: return, goto, task, fanout)
- `args` is a single raw string, which is also why `run_prompt` takes one optional string beside the name
- Substitution namespaces are `args`, `var`, `sys`; `sys` names provenance, not immutability
- A prompt is executed here, against the gateway, and is never published as an MCP prompt
- A prompt is invoked only by name, through `run_prompt`; no prompt is a tool of its own
- A prompt's frontmatter name is the name a caller passes verbatim, never transformed
- The MCP run registry is in memory; recovery from a restart is to fire the prompt again
- Streaming later (Talktron needs it)

## Open questions

- `call` syntax: positional vs table-form
- Context-preserving call: support it or not?
