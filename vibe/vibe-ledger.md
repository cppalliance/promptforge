# Vibe Ledger

## 2026-09-10-1-unified-prompt-model

- Step 1: message record validation in the Lua protocol - `cargo test -p promptforge-lua protocol` - 42 passed, 0 failed (also `cargo test -p promptforge-agent`: 23 passed; clippy and fmt clean).
  - Decision: tool call records require string `id` and `name`, so the guide's legacy `{ id = call.id }` replay shape now errors | Falsifier: the plan's normalized `{id, name, arguments}` record is what `models.loop` appends and step 12 migrates the guide.
  - Decision: cross-record checks (unique call IDs, call-result pairing, alternation) stay out of this parse | Falsifier: step 7 explicitly owns per-dispatch pairing/uniqueness validation.
  - Decision: `ContentPart::ImageUrl` keeps only the URL string, dropping extras like `detail` | Falsifier: the Multimodal contract is data-URI image parts; no prompt or template consumes `detail`, and the type can grow if one does.
- Step 2: `messages.new()` builders module - COMPONENT verify: build, `cargo fmt --check`, clippy clean; `cargo test -p promptforge-lua` - 192 passed + 2 doc-tests, 0 failed.
  - Decision: builder methods live behind the list metatable's `__index` rather than as direct fields, so serde conversion and protocol validation see only plain records | Falsifier: `lua.from_value` on builder output fails or serializes functions.
  - Decision: `install_messages` runs during `inject_host_with_var` beside the H2 models table since the shim needs no privileged captures | Falsifier: a later step requires `messages` in a VM that never injects host values.
  - Decision: used `cargo test -p promptforge-lua` instead of installing cargo-nextest | Falsifier: the project survey lists `cargo test -p <crate>` as an accepted component test command, and installing a global toolchain binary is a heavier, host-level change than the failure warrants.
- Step 3: rename heading-based `execute` to `call` - FOCUSED verify: `cargo build` clean; 29 promptforge-lua and 66 promptforge-core filtered tests passed, 0 failed (full coding run: 192 lua + core suites + 23 agent).
  - Decision: renamed internal depth machinery (`execute_depth` -> `call_depth`, `MAX_EXECUTE_DEPTH` -> `MAX_CALL_DEPTH`) since it is diagnostics-adjacent core naming for this op | Falsifier: any user-visible string or field still renders `execute` for a heading call.
  - Decision: left `guide/`, READMEs, AGENTS.md, and `vibe/` docs untouched; the plan's `migrate-prompts-guides` todo owns doc migration | Falsifier: a doc example that runs in a test still uses heading `execute(`.
  - Decision: kept generic verb uses of "execute" and the `crate::execute` module path unchanged | Falsifier: the executor module itself gets renamed.
- Step 4: namespace-only tool and model invocation - COMPONENT verify: build, `cargo fmt --check`, clippy `-D warnings`, and `cargo test -p promptforge-lua` / `-p promptforge-core` all passed (nextest unavailable, survey fallback used).
  - Decision: internal protocol op string stays `"tool_call"` and Rust variants keep their names; only the Lua-facing surface moved | Falsifier: a later step renames protocol vocabulary to match the namespace.
  - Decision: removed the proxy machinery outright (`wrap_handle`, H1 wrap chunk, `coro_shims` flag, unused `ModelInferHook`) since it existed solely for colon `infer` | Falsifier: a future per-handle Lua-callable shim need reappears.
  - Decision: alias-or-Tool decodes once in `tools/decode.rs::tool_alias`, with the shim passing the raw value through the yield | Falsifier: a consumer needs different error wording per call site.
  - Decision: legacy non-coroutine `models.infer` (hook path, test-only) keeps its single-arg form | Falsifier: the legacy engine is revived for production paths.
  - Pre-existing (deferred): `cargo check -p workshop` fails on a missing staged gateway sidecar binary - environmental setup, untouched by this change.
