# PromptForge Status

## What exists and works

Gateway v0 complete. Two-process vertical slice: `promptforge run` -> the gateway
(which holds the vendor key) -> the backend -> "Hello, world!". The executor no
longer holds any vendor credential, only the gateway URL and shared token.

- `promptforge-core::parser` - frontmatter + H1/description + recursive H2-H6 tree + leading Lua-fence separation. 10 unit tests.
- `promptforge-core::client::GatewayClient` - OpenAI-shaped client pointed at the gateway (URL + shared token; no vendor key).
- `promptforge-core::execute::run` - walks top-level sections in file order, each in a fresh context; a Lua `return` finishes the run early, otherwise prose (if any) takes one round trip and control falls through to the next section. Running off the end yields `default_return`, else the last reply, else a generic completion. 5 unit tests (Lua-only, offline).
- `promptforge-cli` - `promptforge run <file.md>`.
- `promptforge-tool-picker` - in progress. Will become a pure, deterministic, embedding-based engine that resolves a plain-English capability need against an abstract tool catalog (no Lua, MCP, or network). Done: `catalog` - `ToolId` (the `(server, name)` pair, compared as a pair; `qualified_key()` joins with U+001F, which cannot occur in either part), `ToolAnnotations` (optional MCP hints), `ToolDescriptor`, `Catalog` (serde; JSON array of flat MCP-shaped objects), and `enriched_text()` = name + description + top-level `properties` keys sorted ascending for determinism. `config` - `ModelId` (non_exhaustive; v1 has only bge-small-en-v1.5, since only its weights are embedded) and `Config` (serde, every field optional) with justified defaults: `similarity_floor` 0.825 (5% false-bind budget), `duplicate_threshold` 0.98, `top_k` 3, `margin` 0.05 (provisional, not measured); `Config::validate()` rejects NaN or out-of-`0.0..=1.0` thresholds and `top_k` 0, and checks no relation between thresholds: `duplicate_threshold` measures one tool against another while `similarity_floor` measures a need against a tool, so neither bounds the other. `error` - `Error` (thiserror, non_exhaustive) + `Result<T>`. `assets` - the model is compiled into the library: `build.rs` fetches BAAI/bge-small-en-v1.5 via `hf-hub` (blocking) pinned to commit `5c38ec7c`, verifies each file against a hardcoded SHA-256, downcasts the fp32 safetensors to fp16 (133MB -> 67MB, bit-exact round-to-nearest-even; the lone I64 tensor passes through), and stages weights + `tokenizer.json` + `config.json` in `OUT_DIR`, which `include_bytes!` embeds. fp16 is a storage choice only - the loader picks the compute dtype. Nothing model-related is git-visible. First build needs network; later builds use the HF cache, and a stamp file in `OUT_DIR` skips the work. Both failure paths (bad digest, cold cache offline) abort with an actionable message. `embed` - `Embedder` (Send + Sync, manual Debug) loads the compiled-in model once in `Embedder::new` (Candle `VarBuilder::from_buffered_safetensors`, upcast fp16 -> f32 since `include_bytes!` data is 1-byte aligned and Candle's f16 CPU coverage is uneven; `Tokenizer::from_bytes`, truncate at 512, never pad) and `embed(&self, &str) -> Vec<f32>` returns a 384-dimension CLS-pooled, L2-normalized vector. No query prefix: needs and tool texts take the identical path, which is what the study's thresholds were measured under. `embed_all` is a loop, not a batch, so a vector never depends on its neighbours. Failures land on `Error::ModelLoad` / `Tokenize` / `Embed`, each carrying a detail string rather than a dependency's error type. `picker` - `ToolPicker::build(Catalog, Config)` validates the config first (so a bad threshold costs no model load), then loads one `Embedder` and embeds every tool from its `enriched_text()`. Vectors live in one flat contiguous `Vec<f32>` at a 384 stride, one row per tool in catalog order, all unit-norm, so a later cosine is a plain dot product; no persistent cache, memory only. An empty catalog builds rather than erroring - abstention is already an outcome, and a runtime-assembled catalog is legitimately empty. Accessors: `len`/`is_empty`, `tools()`, `config()`, `vector(index)`. `rank` (crate-internal, not public API) - `top_k(query, vectors, k)` scores every row by dot product (both sides are unit-norm, so that is the cosine) and returns `Candidate { index, score }` best first. The sort key is a total order - score descending via `f32::total_cmp`, then catalog position ascending - so a ranking is a function of its inputs even when scores tie exactly; catalog position is the tie-break because it is the only key guaranteed unique (identities may legitimately repeat across servers). A non-finite score orders as the worst possible but is reported as computed; `k` beyond the catalog returns the catalog unpadded; a zero `k`, an empty index, or an empty query returns nothing; truncation is under score-then-position alone, so a candidate a later re-sort would promote is already gone if it missed the cut. `rank::Vectors` is a borrowed view of the flat buffer (`row(i)`, `similarity(a, b)`) that lets the policy compare two tools to each other. `ToolPicker::rank(need, k)` embeds the need and ranks, deciding nothing. `policy` - `Outcome` (public: `Bind(ToolDescriptor)` | `Duplicate(Vec<_>)` | `Ambiguous(Vec<_>)` | `Absent`) and the crate-internal decision function over a ranking + the catalog + the stored vectors + the config. Precedence, first match wins: (1) `Absent` when the top score is below `similarity_floor` - if nothing fits, twin-ness is irrelevant; (2) `Duplicate` when at least one other candidate shares the leader's server and its stored vector is at or above `duplicate_threshold` similar to the leader's, ahead of the margin test so a narrow margin cannot silence a fault; (3) `Bind` when the leader beats the runner-up by at least `margin` (a runner-up above the floor does not weaken it); (4) `Ambiguous` for any remaining near-tie, whatever servers it spans. A twin is a property of the pair of tools, not of their scores: the threshold is the cosine between the two tools' own unit-norm embeddings, so it is the same whatever need is asked, and the duplicate group is not filtered by score. Same-server vs cross-server IS the Duplicate/Ambiguous split, since a descriptor carries no "own vs imported" flag - one server's twin pair is somebody's fixable config error, a cross-server collision is the caller's own intended union. All three thresholds are inclusive (at the floor is considered, a gap equal to the margin binds, a pair exactly at the duplicate threshold is twins); shortlists carry `top_k` candidates but never fewer than two, and `decide` ranks `top_k.max(2)` so a `top_k` of 1 cannot hide a runner-up. Annotation tie-break: among *exactly* equal scores only, prefer `read_only: Some(true)`, then `destructive: Some(false)`, then `idempotent: Some(true)` - a positive claim promotes, and silence is not a claim, so equal or absent hints leave catalog order intact; treating absence as neutral-if-both-present would be intransitive and forfeit determinism. Hints never overturn a score decision: they cannot promote a near-tie to a `Bind` nor rescue a `Duplicate`, and they only reorder the ties that survived truncation - a hint-preferred tool past position k was dropped by the ranking, not demoted by the policy. Public surface, complete: `ToolPicker::resolve(need) -> Result<Outcome>` wires ranking to policy end to end, and `ToolPicker::shortlist(need, k) -> Result<Vec<ToolDescriptor>>` reports the same ordered candidates without judging between them. An abstention is `Ok(Outcome::Absent)`, never an `Err` - an error means the engine could not run (tokenize / forward pass), not that nothing matched. `shortlist` applies `similarity_floor` and nothing else, so it is empty in exactly the cases `resolve` abstains; a caller wanting near-misses lowers the floor, which is the dial for it, rather than getting a different answer from a different method. `k` is authoritative and is not clamped against `Config::top_k` (which bounds only the shortlist a resolution reports): `k` past the catalog returns what there is unpadded, `k` of 0 returns nothing. No `#[allow]` remains anywhere in the crate. 94 unit tests + 11 integration tests through the public API only + a running crate-level doctest, including a golden vector and exact-boundary policy cases on synthetic scores and synthetic stored rows. `tests/fixtures/mixed-servers.json` is a committed five-tool catalog over four servers - one tool with nothing like it, one server's two names for one capability carrying the same description word for word, and one capability published by two servers - and `tests/behavior.rs` loads it through the public serde support via `include_str!`, so no path leaves the crate, and provokes all four outcomes from that prose alone at the default thresholds: a weather need binds, the calendar pair is `Duplicate` (their own embeddings sit at 0.983, over the 0.98 default), the two file tools are `Ambiguous` (0.900 vs 0.875, inside the 0.05 margin), and a translation need is `Absent`. It also pins determinism across the whole pipeline: two builds from one fixture yield identical vectors, outcomes, and shortlists, and repeated calls do not drift. Measured finding: whether a copy-pasted pair is caught depends on how much prose dilutes the differing name - the same two verbatim descriptions under shorter names sit at 0.960, below the default threshold. Suite runs in about 26s (model load dominates; the fixture file's own 9s is two loads, the second being the determinism rebuild). Next: workspace-wide build/clippy/test verification.
- `promptforge-gateway` - axum service: `gateway.toml` (Secret + ${VAR} interpolation), model routing, one OpenAI passthrough upstream, bearer auth, POST /v1/chat/completions, GET /health. Config + routing unit tests + 4 end-to-end tests (fake backend + real client).

Lua + args + substitution. A section's Lua block runs in a sandbox with `args`
(raw input) and `sys` (runtime metadata) exposed and a writable `var` table. A
chunk that returns a plain value finishes the run (no model call); otherwise the
prose is `{{ }}`-substituted and sent to the gateway.

- `promptforge-core::lua::run_chunk` - sandboxed VM (string/table/math + base; no io/os/require/load/debug), instruction hook; exposes `args` + `sys`, returns the top-level value and the `var` table (as JSON). 7 unit tests.
- `promptforge-core::subst::substitute` - resolves `{{ args }}` / `{{ var.x }}` / `{{ sys.x }}` (scalar->string, table->JSON, missing->error, single pass, no formulas). 7 unit tests.
- `sys` (runtime, read-only): `sys.when` (launch, RFC3339), `sys.now` (build snapshot), `sys.id` (context id).
- `execute::run(prompt, args, tools, store)` - fall-through loop over top-level sections; Lua `return` finishes early; else substitute prose -> gateway; run off the end -> `default_return`/last reply/generic. `sys.id` increments per section. Client built lazily. The `store` is a run-scoped `Store` created once by the caller and shared across sections (a Lua `store` table exposes it).
- `prompts/echo.md` (`return args`, no gateway), `prompts/greet.md` (Lua-computed `var` + substitution through the gateway).

## What's next

Deferred and next: live time across turns (`sys.live("now")` replace-in-place tail
refresh, or a `now()` tool); the `facts` bag; then the control-flow tranche - the
rest of the exit rule (goto/task/fanout descriptors), the tool-call loop, and
durable state to carry a non-terminal section's work forward (today an
intermediate section's model reply is not retained). Gateway hardening (admission, pinning, packs,
hot reload, Anthropic shim, streaming) deferred until self-hosted pods exist.

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
- Edition 2024, resolver 3, rust-version 1.89 (floor set by transitive deps of candle and hf-hub)
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
- Exit rule so far: a Lua `return` value ends the run (the return fence); otherwise the section's prose (if any) takes one round trip and control falls through to the next top-level section (fresh context); off the end -> `default_return`, else last reply, else generic
- Substitution namespaces: args (raw string), var (Lua-written), sys (runtime); pure path lookup, no formulas (compute in Lua)
- sys names provenance not immutability: sys.when fixed, sys.now snapshot, sys.id per-context
- Streaming later (Talktron needs it)

## Open questions

- call syntax: positional vs table-form
- Context-preserving call: support it or not?
