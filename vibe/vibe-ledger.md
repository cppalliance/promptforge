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
- Step 5: parser pending Markdown capture - `cargo test -p promptforge-parser` green (87 + 2 doctests); after the review fix, `cargo test -p promptforge-core` green (397 lib + integration), workspace check clean, clippy and fmt clean.
  - Decision: removed `off_walk`/`is_off_walk()` outright rather than leaving always-false stubs, per the decision record rejecting off-walk and reader-only break meanings | Falsifier: a later step reintroduces a thematic-break control meaning.
  - Decision: parser keeps the interleaved Prose/Lua block stream with post-reset prose text; step 6's scheduler accumulates and installs it lazily | Falsifier: step 6 cannot express per-fence lazy prose from the block stream.
  - Decision: H1 shares reset semantics, so `description_text` now comes from below a break | Falsifier: the product contract requires the H1 description from above a break.
  - Decision: carried the minimal executor adjustments in this commit rather than re-add a parser shim (review finding close): the scheduler no longer skips off-walk sections and computes `loop_capable` locally as "last prose block in the section" until step 6 | Falsifier: a shim would require restoring the leading-`---` off-walk parse logic that step 5 deliberately removed, contradicting the plan's no-control-flow-meaning contract.
- Step 6: lazy `prose` and `reply` removal - COMPONENT verify: build, fmt, clippy `-D warnings`, `cargo test -p promptforge-lua` and `-p promptforge-core` all passed (382 core lib + 14 integration + 7 doctests; lua 200 + 2; agent 23).
  - Decision: `_G`-metatable guard over userdata, so `models.infer(prose)` sees a plain string | Falsifier: a caller needs `prose` as a non-string Lua value.
  - Decision: empty buffer installs an empty template (`prose == ""`) | Falsifier: authors need to distinguish "no prose" from "empty prose".
  - Decision: `scope`/`tool_loop` kept as `#[cfg(test)]` modules for step 9 | Falsifier: step 9 rewrites rather than rewires them.
  - Decision: deleted the prompt-level model-tool-loop tests rather than simulating them, because their only trigger (automatic prose inference) was removed and `models.loop` does not exist until step 9 | Falsifier: step 9's `models.loop` tests must re-add equivalent prompt-level coverage (tool scoping, local-tool VM routing, arm soft-degrade).
  - Decision: fixed the clippy `too_many_lines` breakage from the step's own WIP commit (`prose.rs` `install` split into `guard_index`/`guard_newindex`) since the exit criteria require clippy `-D warnings` | Falsifier: the refactor is behavior-preserving - all lua/core tests and clippy pass.
