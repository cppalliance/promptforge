# PromptForge Status

## What exists and works

Gateway v0 complete. Two-process vertical slice: `promptforge run` -> the gateway
(which holds the vendor key) -> the backend -> "Hello, world!". The executor no
longer holds any vendor credential, only the gateway URL and shared token.

- `promptforge-core::parser` - frontmatter + H1/description + recursive H2-H6 tree + leading Lua-fence separation. 10 unit tests.
- `promptforge-core::client::GatewayClient` - OpenAI-shaped client pointed at the gateway (URL + shared token; no vendor key).
- `promptforge-core::execute::run` - walks top-level sections in file order, each in a fresh context; a Lua `return` finishes the run early, otherwise prose (if any) takes one round trip and control falls through to the next section. Running off the end yields `default_return`, else the last reply, else a generic completion. 5 unit tests (Lua-only, offline).
- `promptforge-cli` - `promptforge run <file.md>`.
- `promptforge-tool-picker` - in progress. Will become a pure, deterministic, embedding-based engine that resolves a plain-English capability need against an abstract tool catalog (no Lua, MCP, or network). Done: `catalog` - `ToolId` (the `(server, name)` pair, compared as a pair; `qualified_key()` joins with U+001F, which cannot occur in either part), `ToolAnnotations` (optional MCP hints), `ToolDescriptor`, `Catalog` (serde; JSON array of flat MCP-shaped objects), and `enriched_text()` = name + description + top-level `properties` keys sorted ascending for determinism. `config` - `ModelId` (non_exhaustive; v1 has only bge-small-en-v1.5, since only its weights are embedded) and `Config` (serde, every field optional) with justified defaults: `similarity_floor` 0.825 (5% false-bind budget), `duplicate_threshold` 0.98, `top_k` 3, `margin` 0.05 (provisional, not measured); `Config::validate()` rejects NaN or out-of-`0.0..=1.0` thresholds, `top_k` 0, and a duplicate threshold below the floor. `error` - `Error` (thiserror, non_exhaustive) + `Result<T>`. `assets` - the model is compiled into the library: `build.rs` fetches BAAI/bge-small-en-v1.5 via `hf-hub` (blocking) pinned to commit `5c38ec7c`, verifies each file against a hardcoded SHA-256, downcasts the fp32 safetensors to fp16 (133MB -> 67MB, bit-exact round-to-nearest-even; the lone I64 tensor passes through), and stages weights + `tokenizer.json` + `config.json` in `OUT_DIR`, which `include_bytes!` embeds. fp16 is a storage choice only - the loader picks the compute dtype. Nothing model-related is git-visible. First build needs network; later builds use the HF cache, and a stamp file in `OUT_DIR` skips the work. Both failure paths (bad digest, cold cache offline) abort with an actionable message. `embed` - `Embedder` (Send + Sync, manual Debug) loads the compiled-in model once in `Embedder::new` (Candle `VarBuilder::from_buffered_safetensors`, upcast fp16 -> f32 since `include_bytes!` data is 1-byte aligned and Candle's f16 CPU coverage is uneven; `Tokenizer::from_bytes`, truncate at 512, never pad) and `embed(&self, &str) -> Vec<f32>` returns a 384-dimension CLS-pooled, L2-normalized vector. No query prefix: needs and tool texts take the identical path, which is what the study's thresholds were measured under. `embed_all` is a loop, not a batch, so a vector never depends on its neighbours. Failures land on `Error::ModelLoad` / `Tokenize` / `Embed`, each carrying a detail string rather than a dependency's error type. 37 unit tests including a golden vector. Next: `ToolPicker::build` (embed the catalog once) then ranking and policy.
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
- Exit rule so far: a Lua `return` value ends the run (the return fence); otherwise the section's prose (if any) takes one round trip and control falls through to the next top-level section (fresh context); off the end -> `default_return`, else last reply, else generic
- Substitution namespaces: args (raw string), var (Lua-written), sys (runtime); pure path lookup, no formulas (compute in Lua)
- sys names provenance not immutability: sys.when fixed, sys.now snapshot, sys.id per-context
- Streaming later (Talktron needs it)

## Open questions

- call syntax: positional vs table-form
- Context-preserving call: support it or not?
