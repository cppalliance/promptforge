# PromptForge Status

## What exists and works

Eight crates, five binaries. Production model traffic reaches backends through
the gateway; the explicit core-tests binary runs opt-in real-model scenarios through a temporary `promptforge-gateway` sidecar (gateway owns the pinned 0.6B llama-server). Interactive prompt development uses `promptforge-dev` against an already-running gateway.
The MCP server is complete and serving:
`promptforge-mcp-server serve [--stdio] prompts.toml` runs one of this
repository's own prompts that Cursor or Claude Code names to `run_prompt`.

- `promptforge-core` (lib) - the runtime. It parses required-H1 prompts into ordinary live H1 blocks, an optional explicit `lua shared` library stored as `Prompt.replay`, and alternating lua/prose section blocks. `execute::run` executes H1 live exactly once with `args`, `sys`, `var`, store, inference, and picker-backed capability resolution. `tools.need`, `models.need`, and `models.always` resolve when reached and return frozen first-class Tool and Model objects; skipped conditional branches resolve nothing. Rust captures those objects and serialized H1 `var`. Every section, `execute` call, and fanout arm gets a fresh `SectionVm`: it loads `Prompt.replay` before host injection, installs captured capability handles directly from Rust, injects host APIs and H1-seeded state, then walks alternating blocks. Host APIs are unavailable while the library loads, so top-level side effects fail while function bodies may resolve host globals when later called. Non-final prose is single-shot, final prose runs the full tool loop, and one conversation accumulates per section before being cleared between sections. Lua supports `model:infer`, `execute(target, input?)`, `jump(target)`, and structured `fanout`; `tasks["## Name"]` exposes Section objects. `tools.always` plus pre-prose H2 `tools.add` form tool scope, while `models.use` selects a captured model or inherits `models.always`. Near-duplicate effective tools fail before a model turn. Sealed `sys`, the run store, observer records, opt-in `DebugCapture`, scalar-return precedence, untrusted-result framing, and the `run_chunk` compatibility path remain intact. CLI, dev, core-tests, and MCP callers all pass parsed prompts plus live resolution inputs directly to execution. `design-core.md` is authoritative; `design-core-orig.md` remains byte-identical history.
- `promptforge-core-tests` (bin) - the unpublished author-document and real-model scenario boundary. Three valid, three invalid, and three deterministic offline execution prompts assert public parse contracts, exact constrained Lua checkpoints, scalar prologue return, store fall-through, and concurrent execution-ID partitioning; a separate smoke test covers every shipped prompt. Two explicit live fixtures are compiled into `cargo run -p promptforge-core-tests` (or `... scenarios`). The command writes a temporary gateway profile that pins official Qwen3-0.6B Q8 (SHA-256-pinned), starts `promptforge-gateway serve` on a free loopback port with a random bearer token, waits until `GET /health` and authenticated `GET /v1/models` advertise the local model, then runs the fixtures through `GatewayClient` pointed at that gateway. The gateway owns GGUF download/caching under `~/.promptforge/` and spawns `llama-server`; dropping the guard kills the gateway process tree. Behavioral assertions prove nonempty marked text, one schema-valid call under a local alias distinct from the concrete wire name, deterministic tool-result continuation, a nonempty marked final answer, epilog visibility, and exact one-turn and two-turn budgets. Ordinary tests remain fully offline and never launch gateway or llama-server. Interactive prompt development lives in `promptforge-dev`, not here.
- `promptforge-dev` (bin) - unpublished interactive runner for one prompt against an already-running `promptforge-gateway`. Requires `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_KEY` before parse; never starts gateway or `llama-server`. Args are `<prompt.md> [input] [--watch]` only - context, thinking, and `max_tokens` belong on the prompt under `models.need` / `models.always`. Fetches the live model catalog, prepares the live tools and picker (`web_fetch` always; `web_search` when gateway credentials are present), clears `<prompt-stem>.store/` at start, and passes the parsed prompt plus those resolution inputs directly to execution. It write-through mirrors store files and `.trace/` turn JSON during the run, uses stderr for observer lines, and stdout for the result. Author workflow: `crates/promptforge-dev/README.md`.
- `promptforge-webfetch` (lib) - the `web_fetch` tool: an in-process GET, a readability pass to the article body, and a whole-page markdown fallback, behind a URL and address policy that blocks private ranges and checks redirects. No credential, so it runs wherever the prompt runs. Its stable live identity and complete descriptor are regression-tested. 49 unit tests.
- `promptforge-cli` (bin `promptforge`) - `promptforge run <file.md> [input]`; refuses a file that declares no `promptforge:` version, creates one execution id per invocation before parsing, builds the complete available live registry and a picker catalog from the same instances, then executes the parsed `Prompt` directly. H1 resolves needs live during that run. Local `web_fetch` is always available; gateway-backed `web_search` is omitted without `PROMPTFORGE_GATEWAY_KEY`. One observer reference spans parsing and execution, with `NullObserver` installed by default. `design-cli.md` is its design document and is current. 5 unit tests.
- `promptforge-gateway` (bin) - axum service: `gateway.toml` (`Secret` + `${VAR}` interpolation), model routing with `[[model]]` catalog metadata (`description`, `context`, `thinking`), local-slot `tool_dialect` / `tools_mode` resolved from llama `/props` (HF `<stem>.md` sidecar as template fallback), one OpenAI passthrough upstream, bearer auth, `POST /v1/chat/completions`, `GET /v1/models`, `POST /v1/tools/web_search` (Brave), `GET /health`. `design-gateway.md` is its design document and is current; everything designed and unbuilt stays in the design repository as `design-gateway-residue.md`.
- `promptforge-mcp-server` (bin) - the MCP server, complete. `config` parses strict `prompts.toml`; `catalog` expands includes, excludes, and named exceptions while accumulating faults. The four stable built-ins are `list_prompts`, `run_prompt`, `check_run`, and feature-gated `need_prompt`; prompts are named commands, never separately published tools. The host prepares one semantic picker, complete live registry (`web_fetch`, `web_search`), and model catalog for all runs. `server` reparses the selected source snapshot, admits the run, and executes the resulting `Prompt` directly with live H1 resolution under the same observer and run id. Capability errors surface during execution. `RunResult` reports `run_id`, prompt, status, value, turns, elapsed time, and error in `structuredContent`. Progress forwards exact run and section events over a bounded queue; the registry enforces concurrency, deadlines, retention, and terminal collection. Streamable HTTP serves `/mcp` behind bearer auth with public `/healthz`; stdio binds nothing. Watch reload swaps catalog snapshots without changing the published tool list. Retrieval indexes runnable prompt names and descriptions. `rmcp` is pinned at `=3.1.0`; `--no-default-features` drops `need_prompt`, while picker weights remain required for execution-time live capability resolution. `design-mcp-server.md` is current. 153 unit + 6 binary + 8 integration + 5 doc tests.
  - Every `run_prompt` reuses its existing `run_id` as the execution id, reparses the validated catalog source snapshot under that id, and passes it unchanged through live H1 execution, `RunOptions`, progress logs, and terminal records.
- `promptforge-tool-picker` (lib) - a pure, deterministic, embedding-based engine that resolves a plain-English capability against an abstract tool catalog. `build_with(Arc<Embedder>, Catalog, Config)` is the one indexing path, `build` wraps it, and `rebuild(catalog)` re-indexes over the loaded weights, which is what the MCP server's save-time rebuild rides. bge-small-en-v1.5 is compiled into the library by `build.rs` (pinned commit, hardcoded SHA-256, downcast to fp16, embedded from `OUT_DIR`); the first build anywhere needs Hugging Face access, later ones use its cache. `embed` returns a 384-dimension CLS-pooled unit vector; `resolve` answers `Bind`, `Duplicate`, `Ambiguous`, or `Absent` under `similarity_floor` 0.825, `duplicate_threshold` 0.98, and `margin` 0.05, and `shortlist(need, k)` reports the same ranking without judging it. Abstention is an outcome, never an error. `design-tool-picker.md` is its design document and is current. 105 unit + 13 integration + 8 doc tests.

Prompts in `prompts/`: `echo.md` (`return args`, no gateway), `greet.md` (Lua-computed `var` through the gateway), `hello.md`, `research_person` in `research-person.md`, and `analyst_example` in `analyst-example.md` (`models.need` / `models.use`). The research prompt resolves author-register `search` and `fetch` needs in live H1 Lua and scopes both aliases only in its research section; no repository prompt depends on a concrete tool name. The core-tests smoke test discovers every repository Markdown prompt and requires each to parse, while the MCP server owner test exercises live resolution against its complete registry and a representative model catalog. `gateway.toml` and `prompts.toml` at the root are both working development profiles.

## What's next

Shipped (first-class objects plan): alternating lua/prose blocks, Tool/Model/Section
userdata, `model:infer`, `execute` / `jump`, `tasks[]`, structured fanout
results, `sys.section_name` / `sys.execution` / `sys.section_count` /
`sys.reply_finish_reason`, and `store.exists`.

Deferred: live time across turns (`sys.live("now")` tail refresh, or a `now()`
tool); the `facts` bag; durable state to carry a non-terminal section's work
forward. Gateway hardening (admission, pinning, packs, hot reload, Anthropic
shim, streaming) waits on self-hosted pods. Out of scope for this tranche:
OverlayStore, Lua-as-tool, model file tools, `tools.remove`, store list/grep.

## How to run

```
export ANTHROPIC_API_KEY=sk-ant-...      # the vendor key, only the gateway sees it
export PROMPTFORGE_GATEWAY_KEY=dev-secret      # shared bearer, every process
cargo run -p promptforge-gateway -- serve gateway.toml &

export PROMPTFORGE_GATEWAY_URL=http://127.0.0.1:8081/v1
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
- Model selection comes from prompt `models.use` / `models.always`, not from env or MCP config; non-empty model-facing prose without either binding fails with `Error::ModelRequired`
- Entry point is the first H2; recursive H2-H6 nesting, skipped levels tolerated
- A section ends when the model returns text with no tool calls; 24 round trips per section by default
- `args` is a single raw string, which is also why `run_prompt` takes one optional string beside the name
- Substitution namespaces are `args`, `var`, `sys`; `sys` names provenance, not immutability
- A prompt is executed here, against the gateway, and is never published as an MCP prompt
- A prompt is invoked only by name, through `run_prompt`; no prompt is a tool of its own
- A prompt's frontmatter name is its immutable catalog identity; MCP caller lookup case-folds requests and treats `-` as `_`
- The MCP run registry is in memory; recovery from a restart is to fire the prompt again
- No reranker ships: the spike improved one clean set but regressed TOOLRET below plain bge-small, so domain-specific author-register evidence is required before adding one
- Observer `(execution, section, detail)` triples are authoritative trace records; callers own the stable execution id, recorder synchronization stays in concrete observers, fixed details are payload-free (including length-truncated signals); empty model product fails as `EmptyModelReply` / `Model turn failed` rather than a soft empty-reply detail; constrained `Lua: <message>` checkpoints are the sole author-controlled exception, and any future model-generated label is optional off-path UI text that never replaces or steers the trace
- `DebugCapture` is a separate opt-in on `RunOptions` for raw turn JSON; the observer stays payload-free
- H1 `models.need` resolves live against gateway `GET /v1/models` catalog metadata; H2 `models.use` selects a captured object, and no `models.use` or `models.always` fails with `Error::ModelRequired` for non-empty model-facing prose
- Streaming later (Talktron needs it)
- Tool dialects are auto-resolved (`openai`, `gemma3_tool_code`); per-section `tools.calls` counts are available to Lua after the model
- `execute` is a subroutine (returns reply); `jump` is a context-clearing transfer (no return); fanout arms return structured result objects

## Open questions

- Context-preserving call: support it or not?
